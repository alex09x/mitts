/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

/// Unescapes a Rust string literal token (without the surrounding quotes).
/// Supports standard escapes (\n, \r, \t, \\, \0, \', \"), line continuation (\ + newline),
/// \xNN hex byte escapes (0x00..=0x7F), and \u{H...} unicode escapes.
pub fn unescape_rust_string(raw: &str) -> Option<String> {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\\' {
            let esc = chars.next()?;
            match esc {
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                '\\' => out.push('\\'),
                '0' => out.push('\0'),
                '\'' => out.push('\''),
                '"' => out.push('"'),
                '\n' => {
                    // Line continuation: skip leading whitespace on following line
                    while let Some(&next_ch) = chars.peek() {
                        if next_ch.is_whitespace() {
                            chars.next();
                        } else {
                            break;
                        }
                    }
                }
                'x' => {
                    let h1 = chars.next()?.to_digit(16)?;
                    let h2 = chars.next()?.to_digit(16)?;
                    let byte = ((h1 << 4) | h2) as u8;
                    if byte <= 0x7F {
                        out.push(byte as char);
                    } else {
                        return None;
                    }
                }
                'u' => {
                    if chars.next()? != '{' {
                        return None;
                    }
                    let mut code = 0u32;
                    let mut count = 0;
                    loop {
                        let c = chars.next()?;
                        if c == '}' {
                            break;
                        }
                        if count >= 6 {
                            return None;
                        }
                        let digit = c.to_digit(16)?;
                        code = (code << 4) | digit;
                        count += 1;
                    }
                    if count == 0 {
                        return None;
                    }
                    let unicode_char = char::from_u32(code)?;
                    out.push(unicode_char);
                }
                _ => return None,
            }
        } else {
            out.push(ch);
        }
    }

    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unescape_rust_string() {
        assert_eq!(
            unescape_rust_string(r#"BUILD\x5fMODE"#),
            Some("BUILD_MODE".to_string())
        );
        assert_eq!(
            unescape_rust_string(r#"UNICODE\u{5f}VAR"#),
            Some("UNICODE_VAR".to_string())
        );
        assert_eq!(
            unescape_rust_string(r#"HELLO\nWORLD\t\"\'\\"#),
            Some("HELLO\nWORLD\t\"\'\\".to_string())
        );
        assert_eq!(
            unescape_rust_string("LINE\\\n                CONTINUATION"),
            Some("LINECONTINUATION".to_string())
        );
        assert_eq!(unescape_rust_string(r#"\x80"#), None);
        assert_eq!(unescape_rust_string(r#"\u{invalid}"#), None);
    }
}
