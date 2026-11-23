/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! `package.rust-version`, checked as Cargo does: a build whose units include a package
//! that needs a newer rustc than the one in use fails before compiling anything, unless
//! `--ignore-rust-version` was given.

use std::sync::atomic::{AtomicBool, Ordering};

static IGNORED: AtomicBool = AtomicBool::new(false);

/// `--ignore-rust-version`: skip the check for this process.
pub fn set_ignored(ignored: bool) {
    IGNORED.store(ignored, Ordering::Relaxed);
}

pub fn is_ignored() -> bool {
    IGNORED.load(Ordering::Relaxed)
}

/// (major, minor, patch) of the first `X.Y[.Z]` in `text` (`rustc 1.85.0 (…)` or `1.85`).
/// A pre-release suffix (`1.86.0-nightly`) is dropped: Cargo compares a nightly as its
/// version number.
pub fn parse(text: &str) -> Option<(u64, u64, u64)> {
    let start = text.find(|c: char| c.is_ascii_digit())?;
    let word: String = text[start..]
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let mut parts = word.split('.').filter(|p| !p.is_empty());
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    Some((major, minor, patch))
}

/// A package of the build and the `rust-version` it declares.
pub struct Requirement<'a> {
    pub name: &'a str,
    pub version: &'a str,
    pub rust_version: &'a str,
    pub local: bool,
}

/// Cargo's error when `rustc` (its version text) is older than any requirement; `None`
/// when every package is supported, when the check is ignored, or when a version cannot
/// be read (an unreadable `rust-version` is Cargo's manifest error, not this check's).
pub fn check(rustc: &str, requirements: &[Requirement<'_>]) -> Option<String> {
    if is_ignored() {
        return None;
    }
    let have = parse(rustc)?;
    let mut failing: Vec<&Requirement<'_>> = requirements
        .iter()
        .filter(|r| parse(r.rust_version).is_some_and(|need| need > have))
        .collect();
    if failing.is_empty() {
        return None;
    }
    failing.sort_by(|a, b| (a.name, a.version).cmp(&(b.name, b.version)));
    failing.dedup_by(|a, b| a.name == b.name && a.version == b.version);
    let shown = format!("{}.{}.{}", have.0, have.1, have.2);
    let noun = if failing.len() == 1 {
        "package"
    } else {
        "packages"
    };
    let mut text = format!("rustc {shown} is not supported by the following {noun}:");
    for r in &failing {
        text.push_str(&format!(
            "\n  {}@{} requires rustc {}",
            r.name, r.version, r.rust_version
        ));
    }
    if failing.iter().any(|r| !r.local) {
        text.push_str(&format!(
            "\nEither upgrade rustc or select compatible dependency versions with\n\
             `mitts update <name>@<current-ver> --precise <compatible-ver>`\n\
             where `<compatible-ver>` is the latest version supporting rustc {shown}"
        ));
    }
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_rustc_and_manifest_versions() {
        assert_eq!(
            parse("rustc 1.85.0 (4d91de4e4 2025-02-17)"),
            Some((1, 85, 0))
        );
        assert_eq!(parse("1.86.0-nightly"), Some((1, 86, 0)));
        assert_eq!(parse("1.70"), Some((1, 70, 0)));
        assert_eq!(parse("one"), None);
    }

    #[test]
    fn a_newer_requirement_fails_with_cargos_message() {
        let req = [Requirement {
            name: "app",
            version: "0.1.0",
            rust_version: "1.99",
            local: true,
        }];
        let text = check("rustc 1.85.0", &req).unwrap();
        assert!(text.starts_with("rustc 1.85.0 is not supported by the following package:"));
        assert!(text.contains("app@0.1.0 requires rustc 1.99"));
        assert!(check("rustc 1.99.0", &req).is_none());
        assert!(check("rustc 1.99.0-nightly", &req).is_none());
    }
}
