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
        (Some(a), Some(b)) if a == b => component(&p[1..], &s[1..]),
        _ => false,
    }
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
    }
}
