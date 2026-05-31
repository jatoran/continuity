//! Lightweight Rust source scanning for the conventions checker.
//!
//! Pulled out of `conventions.rs` to keep that file under the 600-line cap.

/// Replace `// ...` line comments with empty space and blank out the
/// contents of `"..."` string literals and `'..'` character literals.
/// Word/pattern checks downstream operate on the result, so e.g.
/// `let s = "panic!(...)";` does not flag the rule.
///
/// Lifetime tokens like `'static` are *not* char literals; they have no
/// closing `'`. We disambiguate by scanning up to 8 chars ahead for a
/// closing quote — if none is found, the `'` is treated as a lifetime
/// introducer and the rest of the line is processed normally.
pub(crate) fn strip_strings_and_comment(line: &str) -> String {
    let chars: Vec<char> = line.chars().collect();
    let mut out = String::with_capacity(line.len());
    let mut in_str = false;
    let mut esc = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if !in_str && c == '/' && chars.get(i + 1) == Some(&'/') {
            break;
        }
        if in_str {
            if esc {
                esc = false;
                out.push(' ');
                i += 1;
                continue;
            }
            if c == '\\' {
                esc = true;
                out.push(' ');
                i += 1;
                continue;
            }
            if c == '"' {
                in_str = false;
                out.push('"');
                i += 1;
                continue;
            }
            out.push(' ');
            i += 1;
            continue;
        }
        if c == '"' {
            in_str = true;
            out.push('"');
            i += 1;
            continue;
        }
        if c == '\'' {
            let mut close = None;
            let mut j = i + 1;
            let mut steps = 0_usize;
            while j < chars.len() && steps < 8 {
                if chars[j] == '\\' {
                    j += 2;
                    steps += 2;
                    continue;
                }
                if chars[j] == '\'' {
                    close = Some(j);
                    break;
                }
                j += 1;
                steps += 1;
            }
            match close {
                Some(end) => {
                    out.push('\'');
                    for _ in (i + 1)..end {
                        out.push(' ');
                    }
                    out.push('\'');
                    i = end + 1;
                }
                None => {
                    out.push('\'');
                    i += 1;
                }
            }
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

/// Return the comment portion of a line (everything after `//`), or empty.
pub(crate) fn comment_text(line: &str) -> String {
    if let Some(idx) = line.find("//") {
        return line[idx + 2..].to_string();
    }
    String::new()
}

pub(crate) fn is_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Substring match honoring word boundaries: the bytes immediately before
/// and after the match must not be identifier characters.
pub(crate) fn find_word(haystack: &str, needle: &str) -> Option<usize> {
    let bytes = haystack.as_bytes();
    let nb = needle.as_bytes();
    if nb.is_empty() || bytes.len() < nb.len() {
        return None;
    }
    let mut i = 0;
    while i + nb.len() <= bytes.len() {
        if &bytes[i..i + nb.len()] == nb {
            let prev_ok = i == 0 || !is_ident_char(bytes[i - 1]);
            let next_idx = i + nb.len();
            let next_ok = next_idx == bytes.len() || !is_ident_char(bytes[next_idx]);
            if prev_ok && next_ok {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_drops_comments_and_blanks_strings() {
        assert_eq!(
            strip_strings_and_comment("let x = \"panic!(\"; // .unwrap()"),
            "let x = \"       \"; "
        );
        assert_eq!(strip_strings_and_comment("// only comment"), "");
        assert_eq!(
            strip_strings_and_comment("doc! { \"hi\" }"),
            "doc! { \"  \" }"
        );
    }

    #[test]
    fn strip_preserves_lifetimes_and_braces() {
        assert_eq!(
            strip_strings_and_comment("fn group(c: &'static str) -> X {"),
            "fn group(c: &'static str) -> X {"
        );
        let s = strip_strings_and_comment("if c == '{' { run() }");
        assert!(s.contains('\''));
        assert!(s.contains('{'));
        assert!(s.contains('}'));
    }

    #[test]
    fn find_word_respects_boundaries() {
        assert_eq!(find_word("use anyhow::Result", "anyhow"), Some(4));
        assert!(find_word("let anyhow_count = 0;", "anyhow").is_none());
    }
}
