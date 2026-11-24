/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Workspace path patterns (`members`, `exclude`) matched component by component.

use std::path::Path;

/// A workspace path pattern against a relative path, component by component: `*` and `?`
/// within a component (in any component, `vendor/*/fixtures`), `**` for any number of
/// components.
pub fn path_glob(pattern: &str, path: &Path) -> bool {
    let pattern: Vec<&str> = pattern
        .trim_start_matches("./")
        .trim_end_matches('/')
        .split('/')
        .filter(|c| !c.is_empty() && *c != ".")
        .collect();
    let path: Vec<String> = path
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    parts(&pattern, &path)
}

fn parts(p: &[&str], s: &[String]) -> bool {
    match (p.first(), s.first()) {
        (None, None) => true,
        (Some(&"**"), _) => parts(&p[1..], s) || (!s.is_empty() && parts(p, &s[1..])),
        (Some(pc), Some(sc)) => {
            let pc: Vec<char> = pc.chars().collect();
            let sc: Vec<char> = sc.chars().collect();
            component(&pc, &sc) && parts(&p[1..], &s[1..])
        }
        _ => false,
    }
}

fn component(p: &[char], s: &[char]) -> bool {
    match (p.first(), s.first()) {
        (None, None) => true,
        (Some('*'), _) => component(&p[1..], s) || (!s.is_empty() && component(p, &s[1..])),
        (Some('?'), Some(_)) => component(&p[1..], &s[1..]),
        (Some('['), Some(c)) => match class(&p[1..], *c) {
            Some((true, rest)) => component(rest, &s[1..]),
            Some((false, _)) => false,
            // No closing `]`: a literal `[`.
            None => *c == '[' && component(&p[1..], &s[1..]),
        },
        (Some(a), Some(b)) if a == b => component(&p[1..], &s[1..]),
        _ => false,
    }
}

/// A `[...]` character class (after the `[`), as the `glob` crate Cargo uses reads it:
/// ranges `a-z`, negation `!` (or `^`), `]` first is literal. Whether `c` matches, and the
/// pattern after the class; `None` when the class is not closed.
fn class(p: &[char], c: char) -> Option<(bool, &[char])> {
    let (negated, mut i) = match p.first() {
        Some('!') | Some('^') => (true, 1),
        _ => (false, 0),
    };
    let mut matched = false;
    let mut first = true;
    while i < p.len() {
        if p[i] == ']' && !first {
            return Some((matched != negated, &p[i + 1..]));
        }
        if i + 2 < p.len() && p[i + 1] == '-' && p[i + 2] != ']' {
            matched |= p[i] <= c && c <= p[i + 2];
            i += 3;
        } else {
            matched |= p[i] == c;
            i += 1;
        }
        first = false;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::path_glob;
    use std::path::Path;

    #[test]
    fn globs_match_in_any_component() {
        assert!(path_glob(
            "vendor/*/fixtures",
            Path::new("vendor/a/fixtures")
        ));
        assert!(path_glob("crates/legacy-*", Path::new("crates/legacy-one")));
        assert!(path_glob("crates/l?gacy", Path::new("crates/legacy")));
        assert!(path_glob("**/fixtures", Path::new("a/b/fixtures")));
        assert!(path_glob("./tools/", Path::new("tools")));
        assert!(!path_glob(
            "vendor/*/fixtures",
            Path::new("vendor/a/b/fixtures")
        ));
        assert!(!path_glob("crates/legacy-*", Path::new("crates/modern")));
        assert!(path_glob("crates/*foo*", Path::new("crates/afoob")));
        assert!(path_glob("crates/[a-c]x", Path::new("crates/bx")));
        assert!(!path_glob("crates/[a-c]x", Path::new("crates/dx")));
        assert!(path_glob("crates/[!a-c]x", Path::new("crates/dx")));
    }
}
