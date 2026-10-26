/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::types::EnvMacroDiscovery;
use super::unescape::unescape_rust_string;
use std::path::Path;

/// Skips whitespace, line comments (//...), and block comments (/*...*/) including nested block comments.
fn skip_trivia(source: &str, mut pos: usize) -> usize {
    let len = source.len();
    while pos < len {
        if source[pos..].starts_with("//") {
            pos += 2;
            while pos < len {
                if let Some(ch) = source[pos..].chars().next() {
                    pos += ch.len_utf8();
                    if ch == '\n' {
                        break;
                    }
                } else {
                    break;
                }
            }
        } else if source[pos..].starts_with("/*") {
            pos += 2;
            let mut depth = 1;
            while pos < len && depth > 0 {
                if source[pos..].starts_with("/*") {
                    depth += 1;
                    pos += 2;
                } else if source[pos..].starts_with("*/") {
                    depth -= 1;
                    pos += 2;
                } else if let Some(ch) = source[pos..].chars().next() {
                    pos += ch.len_utf8();
                } else {
                    break;
                }
            }
        } else if let Some(ch) = source[pos..].chars().next() {
            if ch.is_whitespace() {
                pos += ch.len_utf8();
            } else {
                break;
            }
        } else {
            break;
        }
    }
    pos
}

/// Scans Rust source code for compile-time env!("VAR") and option_env!("VAR") invocations.
/// Advances strictly by UTF-8 character boundaries to prevent panicking on non-ASCII code.
pub fn scan_env_macros(source: &str) -> EnvMacroDiscovery {
    let len = source.len();
    let mut i = 0;
    let mut vars = Vec::new();

    while i < len {
        // 1. Skip line comments
        if source[i..].starts_with("//") {
            i += 2;
            while i < len {
                if let Some(ch) = source[i..].chars().next() {
                    i += ch.len_utf8();
                    if ch == '\n' {
                        break;
                    }
                } else {
                    break;
                }
            }
            continue;
        }

        // 2. Skip block comments (handling nested comments)
        if source[i..].starts_with("/*") {
            i += 2;
            let mut depth = 1;
            while i < len && depth > 0 {
                if source[i..].starts_with("/*") {
                    depth += 1;
                    i += 2;
                } else if source[i..].starts_with("*/") {
                    depth -= 1;
                    i += 2;
                } else if let Some(ch) = source[i..].chars().next() {
                    i += ch.len_utf8();
                } else {
                    break;
                }
            }
            continue;
        }

        // 3. Skip raw strings: r#"..."# or r"..."
        let raw_prefix = if source[i..].starts_with("r#") || source[i..].starts_with("r\"") {
            Some(1)
        } else if source[i..].starts_with("br#")
            || source[i..].starts_with("br\"")
            || source[i..].starts_with("cr#")
            || source[i..].starts_with("cr\"")
        {
            Some(2)
        } else {
            None
        };
        if let Some(pref_len) = raw_prefix {
            let rest = &source[i + pref_len..];
            let mut hashes = 0;
            while rest[hashes..].starts_with('#') {
                hashes += 1;
            }
            if rest[hashes..].starts_with('"') {
                let content_start = i + pref_len + hashes + 1;
                let closing = format!("\"{}", "#".repeat(hashes));
                if let Some(pos) = source[content_start..].find(&closing) {
                    i = content_start + pos + closing.len();
                    continue;
                } else {
                    break;
                }
            }
        }

        // 4. Skip normal string literals: "..." (or b"...", c"...")
        let str_prefix = if source[i..].starts_with('"') {
            Some(1)
        } else if source[i..].starts_with("b\"") || source[i..].starts_with("c\"") {
            Some(2)
        } else {
            None
        };
        if let Some(pref_len) = str_prefix {
            i += pref_len;
            let mut escaped = false;
            while i < len {
                if let Some(ch) = source[i..].chars().next() {
                    i += ch.len_utf8();
                    if escaped {
                        escaped = false;
                    } else if ch == '\\' {
                        escaped = true;
                    } else if ch == '"' {
                        break;
                    }
                } else {
                    break;
                }
            }
            continue;
        }

        // 5. Check for compile-time env! or option_env! invocation
        let (ident_len, is_macro_ident) = if source[i..].starts_with("option_env") {
            let next_ch = source[i + 10..].chars().next();
            let is_ident_boundary = next_ch
                .map(|c| !c.is_alphanumeric() && c != '_')
                .unwrap_or(true);
            (10, is_ident_boundary)
        } else if source[i..].starts_with("env") {
            let next_ch = source[i + 3..].chars().next();
            let is_ident_boundary = next_ch
                .map(|c| !c.is_alphanumeric() && c != '_')
                .unwrap_or(true);
            (3, is_ident_boundary)
        } else {
            (0, false)
        };

        if is_macro_ident {
            if i > 0 {
                if let Some(prev) = source[..i].chars().next_back() {
                    if prev.is_alphanumeric() || prev == '_' {
                        i += ident_len;
                        continue;
                    }
                }
            }

            let after_ident = i + ident_len;
            let bang_pos = skip_trivia(source, after_ident);

            if bang_pos < len
                && source[bang_pos..].starts_with('!')
                && !source[bang_pos..].starts_with("!=")
            {
                let delim_pos = skip_trivia(source, bang_pos + 1);
                if delim_pos < len {
                    let delim = source[delim_pos..].chars().next();
                    if delim == Some('(') || delim == Some('[') || delim == Some('{') {
                        let mut j = delim_pos + 1;
                        j = skip_trivia(source, j);

                        if source[j..].starts_with('"') {
                            j += 1;
                            let str_start = j;
                            let mut escaped = false;
                            let mut str_end = None;
                            while j < len {
                                if let Some(ch) = source[j..].chars().next() {
                                    j += ch.len_utf8();
                                    if escaped {
                                        escaped = false;
                                    } else if ch == '\\' {
                                        escaped = true;
                                    } else if ch == '"' {
                                        str_end = Some(j - 1);
                                        break;
                                    }
                                } else {
                                    break;
                                }
                            }

                            if let Some(end) = str_end {
                                let raw_token = &source[str_start..end];
                                if let Some(unescaped) = unescape_rust_string(raw_token) {
                                    vars.push(unescaped);
                                    i = j;
                                    continue;
                                } else {
                                    return EnvMacroDiscovery::UntrackedDynamic;
                                }
                            } else {
                                return EnvMacroDiscovery::UntrackedDynamic;
                            }
                        } else if source[j..].starts_with("r#") || source[j..].starts_with("r\"") {
                            let pref_len = 1;
                            let rest = &source[j + pref_len..];
                            let mut hashes = 0;
                            while rest[hashes..].starts_with('#') {
                                hashes += 1;
                            }
                            if rest[hashes..].starts_with('"') {
                                let content_start = j + pref_len + hashes + 1;
                                let closing = format!("\"{}", "#".repeat(hashes));
                                if let Some(pos) = source[content_start..].find(&closing) {
                                    let raw_token = &source[content_start..content_start + pos];
                                    vars.push(raw_token.to_string());
                                    i = content_start + pos + closing.len();
                                    continue;
                                } else {
                                    return EnvMacroDiscovery::UntrackedDynamic;
                                }
                            } else {
                                return EnvMacroDiscovery::UntrackedDynamic;
                            }
                        } else if source[j..].starts_with('#') {
                            // `env!(#var)` is `quote!` interpolation: a proc macro emitting
                            // the call for another crate, not a read of this crate's env.
                            i = j;
                            continue;
                        } else {
                            return EnvMacroDiscovery::UntrackedDynamic;
                        }
                    }
                }
            }
        }

        if let Some(ch) = source[i..].chars().next() {
            i += ch.len_utf8();
        } else {
            break;
        }
    }

    vars.sort();
    vars.dedup();
    EnvMacroDiscovery::Tracked(vars)
}

/// Recursively scans a package directory for compile-time env!/option_env! usages across
/// all .rs files, reusing the per-file memo in `env_macro_cache` for unchanged files.
pub fn discover_package_env_macros(package_dir: &Path) -> (EnvMacroDiscovery, Vec<String>) {
    super::env_macro_cache::discover(package_dir)
}

#[cfg(test)]
#[path = "env_macro_tests.rs"]
mod tests;
