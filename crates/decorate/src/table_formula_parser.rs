//! Tokenizer + recursive-descent parser for the Phase-F4 inline
//! table formula language.
//!
//! Split out of [`crate::table_formula`] so that file stays under the
//! 600-line cap. The grammar (precedence: unary minus > `* /` >
//! `+ -`) and surface types stay co-located with the evaluator in
//! the parent module; this file owns only the token stream + parse
//! tree construction.
//!
//! Thread ownership: stateless, callable from any thread.

use crate::table_formula::{CellRef, Expr, FormulaError, FuncKind, Op};

/// Token stream consumed by the recursive-descent parser. Only used
/// internally — the public surface is [`parse`].
#[derive(Clone, Debug, PartialEq)]
pub enum Token {
    /// Numeric literal.
    Num(f64),
    /// Bare identifier (a function name like `SUM`).
    Ident(String),
    /// `A1`-style cell reference (column letters + row digits).
    Cell(CellRef),
    /// `(`
    LParen,
    /// `)`
    RParen,
    /// `:` — range separator inside function calls.
    Colon,
    /// `,` — reserved; the language has no comma-separated
    /// argument lists today, but the lexer surfaces it so future
    /// extensions don't break the tokenizer.
    Comma,
    /// `+`
    Plus,
    /// `-`
    Minus,
    /// `*`
    Star,
    /// `/`
    Slash,
}

/// Parse `s` (already stripped of any leading `=`). Returns the
/// expression tree or a [`FormulaError::Syntax`] for any tokenizer
/// or parse failure.
///
/// # Errors
/// Propagates every [`FormulaError`] the tokenizer or parser emits.
pub fn parse(s: &str) -> Result<Expr, FormulaError> {
    let tokens = tokenize(s)?;
    let mut parser = Parser { tokens, pos: 0 };
    let expr = parser.parse_expr()?;
    if parser.pos < parser.tokens.len() {
        return Err(FormulaError::Syntax("trailing input"));
    }
    Ok(expr)
}

fn tokenize(s: &str) -> Result<Vec<Token>, FormulaError> {
    let mut out = Vec::new();
    let mut chars = s.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
            continue;
        }
        if c.is_ascii_digit() || c == '.' {
            let mut buf = String::new();
            while let Some(&c) = chars.peek() {
                if c.is_ascii_digit() || c == '.' {
                    buf.push(c);
                    chars.next();
                } else {
                    break;
                }
            }
            let n: f64 = buf.parse().map_err(|_| FormulaError::Syntax("number"))?;
            out.push(Token::Num(n));
            continue;
        }
        if c.is_ascii_alphabetic() {
            let mut buf = String::new();
            while let Some(&c) = chars.peek() {
                if c.is_ascii_alphabetic() {
                    buf.push(c);
                    chars.next();
                } else {
                    break;
                }
            }
            // Could be `A1` (cell ref) or `SUM` (ident).
            let mut digits = String::new();
            while let Some(&c) = chars.peek() {
                if c.is_ascii_digit() {
                    digits.push(c);
                    chars.next();
                } else {
                    break;
                }
            }
            if !digits.is_empty() {
                let col = letters_to_col(&buf).ok_or(FormulaError::Syntax("cell column"))?;
                let row: u32 = digits
                    .parse()
                    .map_err(|_| FormulaError::Syntax("cell row"))?;
                if row == 0 {
                    return Err(FormulaError::Syntax("row is 1-indexed"));
                }
                out.push(Token::Cell(CellRef { col, row: row - 1 }));
            } else {
                out.push(Token::Ident(buf.to_ascii_uppercase()));
            }
            continue;
        }
        let t = match c {
            '(' => Token::LParen,
            ')' => Token::RParen,
            ':' => Token::Colon,
            ',' => Token::Comma,
            '+' => Token::Plus,
            '-' => Token::Minus,
            '*' => Token::Star,
            '/' => Token::Slash,
            _ => return Err(FormulaError::Syntax("character")),
        };
        out.push(t);
        chars.next();
    }
    Ok(out)
}

pub fn letters_to_col(s: &str) -> Option<u32> {
    if s.is_empty() || !s.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    let mut col: u32 = 0;
    for c in s.chars() {
        col = col.checked_mul(26)?;
        col = col.checked_add((c.to_ascii_uppercase() as u32) - ('A' as u32) + 1)?;
    }
    Some(col - 1)
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn bump(&mut self) -> Option<Token> {
        let t = self.tokens.get(self.pos)?.clone();
        self.pos += 1;
        Some(t)
    }

    fn parse_expr(&mut self) -> Result<Expr, FormulaError> {
        self.parse_add()
    }

    fn parse_add(&mut self) -> Result<Expr, FormulaError> {
        let mut lhs = self.parse_mul()?;
        while let Some(tok) = self.peek() {
            let op = match tok {
                Token::Plus => Op::Add,
                Token::Minus => Op::Sub,
                _ => break,
            };
            self.bump();
            let rhs = self.parse_mul()?;
            lhs = Expr::Bin(op, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn parse_mul(&mut self) -> Result<Expr, FormulaError> {
        let mut lhs = self.parse_unary()?;
        while let Some(tok) = self.peek() {
            let op = match tok {
                Token::Star => Op::Mul,
                Token::Slash => Op::Div,
                _ => break,
            };
            self.bump();
            let rhs = self.parse_unary()?;
            lhs = Expr::Bin(op, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn parse_unary(&mut self) -> Result<Expr, FormulaError> {
        if matches!(self.peek(), Some(Token::Minus)) {
            self.bump();
            let inner = self.parse_unary()?;
            return Ok(Expr::Neg(Box::new(inner)));
        }
        self.parse_atom()
    }

    fn parse_atom(&mut self) -> Result<Expr, FormulaError> {
        let tok = self.bump().ok_or(FormulaError::Syntax("end of input"))?;
        match tok {
            Token::Num(n) => Ok(Expr::Number(n)),
            Token::Cell(cell) => Ok(Expr::Ref(cell)),
            Token::LParen => {
                let inner = self.parse_expr()?;
                if !matches!(self.bump(), Some(Token::RParen)) {
                    return Err(FormulaError::Syntax("missing )"));
                }
                Ok(inner)
            }
            Token::Ident(name) => {
                let kind = FuncKind::from_str(&name).ok_or(FormulaError::Syntax("function"))?;
                if !matches!(self.bump(), Some(Token::LParen)) {
                    return Err(FormulaError::Syntax("missing ("));
                }
                let start = match self.bump() {
                    Some(Token::Cell(c)) => c,
                    _ => return Err(FormulaError::Syntax("cell start")),
                };
                if !matches!(self.bump(), Some(Token::Colon)) {
                    return Err(FormulaError::Syntax("missing :"));
                }
                let end = match self.bump() {
                    Some(Token::Cell(c)) => c,
                    _ => return Err(FormulaError::Syntax("cell end")),
                };
                if !matches!(self.bump(), Some(Token::RParen)) {
                    return Err(FormulaError::Syntax("missing )"));
                }
                Ok(Expr::Range(kind, start, end))
            }
            _ => Err(FormulaError::Syntax("atom")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_to_col_round_trips_aa() {
        assert_eq!(letters_to_col("A"), Some(0));
        assert_eq!(letters_to_col("Z"), Some(25));
        assert_eq!(letters_to_col("AA"), Some(26));
        assert_eq!(letters_to_col("AZ"), Some(51));
        assert_eq!(letters_to_col("BA"), Some(52));
    }
}
