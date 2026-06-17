//! Arithmetic expression evaluator for the command palette (E2).
//!
//! Typing an arithmetic expression into the palette filter line surfaces
//! the evaluated result inline at the top of the result list. The
//! expression grammar is small and entirely local — no scripting engine,
//! no settings dependency, no network. The evaluator owns its own
//! recursive-descent parser to avoid pulling in a third-party crate
//! (spec §16.4: anti-bloat).
//!
//! ## Operators (low → high precedence)
//! - `|`  bitwise or (operands cast to `i64`)
//! - `&`  bitwise and
//! - `<<` `>>`  bit shifts
//! - `+` `-`
//! - `*` `/` `%`
//! - unary `+` `-`
//! - `^`  power, right-associative
//!
//! `^` is power, not bitwise-xor. The §E2 spec lists both groups but
//! `^` only makes sense in one slot for a calculator; power wins.
//!
//! ## Atoms
//! - Decimal literal: `12`, `3.14`, `2e10`, `1.5e-3`.
//! - Hex literal: `0x1F`, `0xff`.
//! - Constants (case-insensitive): `pi`, `e`.
//! - Function calls (case-insensitive): `sqrt`, `abs`, `floor`, `ceil`,
//!   `round` (unary); `min`, `max` (variadic, ≥1 arg).
//! - Parenthesised sub-expression.
//!
//! ## Intent detection
//! [`is_math_intent`] returns `true` when the first non-whitespace
//! character is a digit, `.`, `(`, `+`, or `-`. This deliberately avoids
//! eating command names like `editor.find` or `view.pick_theme`.
//!
//! Thread ownership: stateless pure functions; safe to call from the UI
//! thread on every [`crate::palette::Palette::refilter`] tick.

/// Result of evaluating the palette's filter line as an arithmetic
/// expression.
#[derive(Clone, Debug, PartialEq)]
pub struct MathPreview {
    /// Verbatim expression as typed (trimmed of leading/trailing
    /// whitespace) — used for the rendered row label `expr = value`.
    pub expr: String,
    /// Evaluated value. Always finite (NaN / inf preview is dropped).
    pub value: f64,
}

/// `true` when `input` looks like it might be an arithmetic expression
/// rather than a command-name fuzzy filter.
#[must_use]
pub(crate) fn is_math_intent(input: &str) -> bool {
    match input.trim_start().chars().next() {
        Some(c) => matches!(c, '0'..='9' | '.' | '(' | '+' | '-'),
        None => false,
    }
}

/// Try to evaluate `input` as an arithmetic expression. Returns `Some`
/// only on a fully-consumed parse with a finite numeric result.
#[must_use]
pub(crate) fn try_eval(input: &str) -> Option<f64> {
    let bytes = input.as_bytes();
    let mut p = Parser { src: bytes, pos: 0 };
    let v = p.expr()?;
    p.skip_ws();
    if p.pos != p.src.len() {
        return None;
    }
    if !v.is_finite() {
        return None;
    }
    Some(v)
}

/// Try to build a [`MathPreview`] from `input`. Combines
/// [`is_math_intent`] + [`try_eval`].
#[must_use]
pub fn preview(input: &str) -> Option<MathPreview> {
    if !is_math_intent(input) {
        return None;
    }
    let value = try_eval(input)?;
    Some(MathPreview {
        expr: input.trim().to_string(),
        value,
    })
}

/// Render a `f64` for display + clipboard. Integer-valued numbers
/// (within ±2^53) render as plain integers; others get up to 12 decimal
/// digits with trailing zeros trimmed.
#[must_use]
pub(crate) fn format_value(v: f64) -> String {
    if !v.is_finite() {
        return "—".to_string();
    }
    if v == v.trunc() && v.abs() < 9_007_199_254_740_992.0 {
        return format!("{}", v as i64);
    }
    let s = format!("{v:.12}");
    let trimmed = s.trim_end_matches('0').trim_end_matches('.');
    if trimmed.is_empty() || trimmed == "-" {
        "0".to_string()
    } else {
        trimmed.to_string()
    }
}

struct Parser<'a> {
    src: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn skip_ws(&mut self) {
        while self.pos < self.src.len() && self.src[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
    }
    fn peek_byte(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }
    fn consume_byte(&mut self, b: u8) -> bool {
        self.skip_ws();
        if self.peek_byte() == Some(b) {
            self.pos += 1;
            true
        } else {
            false
        }
    }
    fn consume_two(&mut self, b1: u8, b2: u8) -> bool {
        self.skip_ws();
        if self.src.get(self.pos) == Some(&b1) && self.src.get(self.pos + 1) == Some(&b2) {
            self.pos += 2;
            true
        } else {
            false
        }
    }

    fn expr(&mut self) -> Option<f64> {
        self.bitor()
    }
    fn bitor(&mut self) -> Option<f64> {
        let mut left = self.bitand()?;
        loop {
            self.skip_ws();
            if self.peek_byte() == Some(b'|') {
                self.pos += 1;
                let right = self.bitand()?;
                left = ((left as i64) | (right as i64)) as f64;
            } else {
                break;
            }
        }
        Some(left)
    }
    fn bitand(&mut self) -> Option<f64> {
        let mut left = self.shift()?;
        loop {
            self.skip_ws();
            if self.peek_byte() == Some(b'&') {
                self.pos += 1;
                let right = self.shift()?;
                left = ((left as i64) & (right as i64)) as f64;
            } else {
                break;
            }
        }
        Some(left)
    }
    fn shift(&mut self) -> Option<f64> {
        let mut left = self.addsub()?;
        loop {
            if self.consume_two(b'<', b'<') {
                let r = self.addsub()?;
                let shift = (r as i64) & 63;
                left = ((left as i64).wrapping_shl(shift as u32)) as f64;
            } else if self.consume_two(b'>', b'>') {
                let r = self.addsub()?;
                let shift = (r as i64) & 63;
                left = ((left as i64).wrapping_shr(shift as u32)) as f64;
            } else {
                break;
            }
        }
        Some(left)
    }
    fn addsub(&mut self) -> Option<f64> {
        let mut left = self.muldiv()?;
        loop {
            self.skip_ws();
            match self.peek_byte() {
                Some(b'+') => {
                    self.pos += 1;
                    let r = self.muldiv()?;
                    left += r;
                }
                Some(b'-') => {
                    self.pos += 1;
                    let r = self.muldiv()?;
                    left -= r;
                }
                _ => break,
            }
        }
        Some(left)
    }
    fn muldiv(&mut self) -> Option<f64> {
        let mut left = self.unary()?;
        loop {
            self.skip_ws();
            match self.peek_byte() {
                Some(b'*') => {
                    self.pos += 1;
                    let r = self.unary()?;
                    left *= r;
                }
                Some(b'/') => {
                    self.pos += 1;
                    let r = self.unary()?;
                    if r == 0.0 {
                        return None;
                    }
                    left /= r;
                }
                Some(b'%') => {
                    self.pos += 1;
                    let r = self.unary()?;
                    if r == 0.0 {
                        return None;
                    }
                    left %= r;
                }
                _ => break,
            }
        }
        Some(left)
    }
    fn unary(&mut self) -> Option<f64> {
        self.skip_ws();
        match self.peek_byte() {
            Some(b'-') => {
                self.pos += 1;
                Some(-self.unary()?)
            }
            Some(b'+') => {
                self.pos += 1;
                self.unary()
            }
            _ => self.power(),
        }
    }
    fn power(&mut self) -> Option<f64> {
        let base = self.atom()?;
        self.skip_ws();
        if self.peek_byte() == Some(b'^') {
            self.pos += 1;
            let exp = self.unary()?;
            Some(base.powf(exp))
        } else {
            Some(base)
        }
    }
    fn atom(&mut self) -> Option<f64> {
        self.skip_ws();
        let c = self.peek_byte()?;
        if c == b'(' {
            self.pos += 1;
            let v = self.expr()?;
            if !self.consume_byte(b')') {
                return None;
            }
            return Some(v);
        }
        if c.is_ascii_digit() || c == b'.' {
            return self.number();
        }
        if c.is_ascii_alphabetic() || c == b'_' {
            return self.ident_or_call();
        }
        None
    }
    fn number(&mut self) -> Option<f64> {
        if self.src.get(self.pos) == Some(&b'0')
            && matches!(self.src.get(self.pos + 1), Some(&b'x') | Some(&b'X'))
        {
            self.pos += 2;
            let start = self.pos;
            while self.pos < self.src.len() && self.src[self.pos].is_ascii_hexdigit() {
                self.pos += 1;
            }
            if self.pos == start {
                return None;
            }
            let s = std::str::from_utf8(&self.src[start..self.pos]).ok()?;
            return i64::from_str_radix(s, 16).ok().map(|i| i as f64);
        }
        let start = self.pos;
        let mut seen_digit = false;
        let mut seen_dot = false;
        while self.pos < self.src.len() {
            let b = self.src[self.pos];
            if b.is_ascii_digit() {
                seen_digit = true;
                self.pos += 1;
            } else if b == b'.' && !seen_dot {
                seen_dot = true;
                self.pos += 1;
            } else if (b == b'e' || b == b'E') && seen_digit {
                self.pos += 1;
                if matches!(self.src.get(self.pos), Some(&b'+') | Some(&b'-')) {
                    self.pos += 1;
                }
                while self.pos < self.src.len() && self.src[self.pos].is_ascii_digit() {
                    self.pos += 1;
                }
                break;
            } else {
                break;
            }
        }
        if !seen_digit {
            return None;
        }
        let s = std::str::from_utf8(&self.src[start..self.pos]).ok()?;
        s.parse::<f64>().ok()
    }
    fn ident_or_call(&mut self) -> Option<f64> {
        let start = self.pos;
        while self.pos < self.src.len()
            && (self.src[self.pos].is_ascii_alphanumeric() || self.src[self.pos] == b'_')
        {
            self.pos += 1;
        }
        let name = std::str::from_utf8(&self.src[start..self.pos]).ok()?;
        let lower = name.to_ascii_lowercase();
        match lower.as_str() {
            "pi" => return Some(std::f64::consts::PI),
            "e" => return Some(std::f64::consts::E),
            _ => {}
        }
        self.skip_ws();
        if self.peek_byte() != Some(b'(') {
            return None;
        }
        self.pos += 1;
        let mut args: Vec<f64> = Vec::new();
        self.skip_ws();
        if self.peek_byte() != Some(b')') {
            loop {
                let v = self.expr()?;
                args.push(v);
                self.skip_ws();
                if self.peek_byte() == Some(b',') {
                    self.pos += 1;
                    continue;
                }
                break;
            }
        }
        if !self.consume_byte(b')') {
            return None;
        }
        match (lower.as_str(), args.len()) {
            ("sqrt", 1) => Some(args[0].sqrt()),
            ("abs", 1) => Some(args[0].abs()),
            ("floor", 1) => Some(args[0].floor()),
            ("ceil", 1) => Some(args[0].ceil()),
            ("round", 1) => Some(args[0].round()),
            ("min", n) if n >= 1 => Some(args.into_iter().fold(f64::INFINITY, f64::min)),
            ("max", n) if n >= 1 => Some(args.into_iter().fold(f64::NEG_INFINITY, f64::max)),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intent_detected_on_leading_digit_or_op() {
        assert!(is_math_intent("5+3"));
        assert!(is_math_intent("  (1+2)"));
        assert!(is_math_intent("-5"));
        assert!(is_math_intent("+7"));
        assert!(is_math_intent(".5"));
        assert!(is_math_intent("0x1F"));
    }

    #[test]
    fn intent_ignored_for_command_names() {
        assert!(!is_math_intent("editor.find"));
        assert!(!is_math_intent("sqrt(2)")); // doesn't match leading-char rule
        assert!(!is_math_intent("view.zoom"));
        assert!(!is_math_intent(""));
        assert!(!is_math_intent("   "));
    }

    #[test]
    fn addition_and_precedence() {
        assert_eq!(try_eval("1+2"), Some(3.0));
        assert_eq!(try_eval("2+3*4"), Some(14.0));
        assert_eq!(try_eval("(2+3)*4"), Some(20.0));
        assert_eq!(try_eval("10 - 4 - 2"), Some(4.0)); // left-assoc
    }

    #[test]
    fn power_right_assoc() {
        assert_eq!(try_eval("2^3"), Some(8.0));
        // 2^(3^2) = 2^9 = 512, not (2^3)^2 = 64
        assert_eq!(try_eval("2^3^2"), Some(512.0));
        assert_eq!(try_eval("-2^2"), Some(-4.0)); // unary binds outside power → -(2^2)
    }

    #[test]
    fn division_and_modulo() {
        assert_eq!(try_eval("10/4"), Some(2.5));
        assert_eq!(try_eval("10%3"), Some(1.0));
        assert!(try_eval("1/0").is_none());
        assert!(try_eval("1%0").is_none());
    }

    #[test]
    fn unary_chain() {
        assert_eq!(try_eval("--5"), Some(5.0));
        assert_eq!(try_eval("-+-5"), Some(5.0));
    }

    #[test]
    fn constants() {
        let pi = try_eval("pi").unwrap();
        assert!((pi - std::f64::consts::PI).abs() < 1e-12);
        let e = try_eval("e").unwrap();
        assert!((e - std::f64::consts::E).abs() < 1e-12);
    }

    #[test]
    fn functions_unary() {
        assert_eq!(try_eval("sqrt(16)"), Some(4.0));
        assert_eq!(try_eval("abs(-7)"), Some(7.0));
        assert_eq!(try_eval("floor(3.7)"), Some(3.0));
        assert_eq!(try_eval("ceil(3.1)"), Some(4.0));
        assert_eq!(try_eval("round(3.5)"), Some(4.0));
    }

    #[test]
    fn functions_variadic_min_max() {
        assert_eq!(try_eval("min(3,1,2)"), Some(1.0));
        assert_eq!(try_eval("max(3,1,2)"), Some(3.0));
        assert_eq!(try_eval("min(5)"), Some(5.0));
    }

    #[test]
    fn hex_literals() {
        assert_eq!(try_eval("0xff"), Some(255.0));
        assert_eq!(try_eval("0x10 + 1"), Some(17.0));
    }

    #[test]
    fn bitwise() {
        assert_eq!(try_eval("0xff & 0x0f"), Some(0x0f as f64));
        assert_eq!(try_eval("0x10 | 0x01"), Some(0x11 as f64));
        assert_eq!(try_eval("1 << 4"), Some(16.0));
        assert_eq!(try_eval("256 >> 2"), Some(64.0));
    }

    #[test]
    fn rejects_trailing_garbage() {
        assert!(try_eval("1+2 garbage").is_none());
        assert!(try_eval("(1+2").is_none());
        assert!(try_eval("min(").is_none());
    }

    #[test]
    fn preview_round_trip() {
        let p = preview("  3 + 4 ").unwrap();
        assert_eq!(p.value, 7.0);
        assert_eq!(p.expr, "3 + 4");
        assert!(preview("editor.find").is_none());
    }

    #[test]
    fn format_integer_when_round() {
        assert_eq!(format_value(7.0), "7");
        assert_eq!(format_value(-0.0), "0");
        assert_eq!(format_value(255.0), "255");
    }

    #[test]
    fn format_decimal_trims_trailing_zeros() {
        assert_eq!(format_value(2.5), "2.5");
        assert_eq!(format_value(1.25), "1.25");
        // pi ~ 3.141592653589793
        let s = format_value(std::f64::consts::PI);
        assert!(s.starts_with("3.14159"), "got {s}");
        assert!(!s.ends_with('0'), "trailing zeros not trimmed: {s}");
    }

    #[test]
    fn format_nonfinite_safe() {
        assert_eq!(format_value(f64::NAN), "—");
        assert_eq!(format_value(f64::INFINITY), "—");
    }
}
