/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Workspace `members` and `exclude` as Cargo reads them. `members` entries are glob
//! patterns expanded with the `glob` crate Cargo uses (`*`, `?`, `**`, `[...]` with `!`
//! negation, invalid patterns refused). `exclude` entries are paths, not patterns: a package
//! is excluded when its directory lies under one, unless a literal `members` entry names a
//! directory it lies under (`WorkspaceRootConfig::is_excluded` in Cargo).

use anyhow::{Context, Result};
use std::path::{Component, Path, PathBuf};

fn is_pattern(entry: &str) -> bool {
    entry.contains(['*', '?', '['])
}

/// `entry` as relative path components (`./a/b/` -> `a/b`, `.` -> ``).
fn normalized(entry: &str) -> PathBuf {
    Path::new(entry)
        .components()
        .filter(|c| !matches!(c, Component::CurDir))
        .collect()
}

/// Refuses a `members` pattern Cargo's glob parser refuses (`crates/[abc`).
pub fn validate(entry: &str) -> Result<()> {
    if is_pattern(entry) {
        glob::Pattern::new(entry)
            .map(|_| ())
            .with_context(|| format!("invalid glob pattern for workspace member `{entry}`"))?;
    }
    Ok(())
}

/// The directories a `members` entry names under `root`: a literal path as given, a pattern
/// expanded on the file system as `glob::glob` does (following symlinks, any depth for
/// `**`), sorted.
pub fn expand(root: &Path, entry: &str) -> Result<Vec<PathBuf>> {
    if !is_pattern(entry) {
        return Ok(vec![root.join(normalized(entry))]);
    }
    // The root is a literal path: its own `[`, `*` or `?` are not part of the pattern.
    let root_text = root
        .to_str()
        .with_context(|| format!("workspace root is not UTF-8: {}", root.display()))?;
    let full = format!(
        "{}/{}",
        glob::Pattern::escape(root_text.trim_end_matches('/')),
        entry.trim_start_matches("./")
    );
    let mut dirs: Vec<PathBuf> = Vec::new();
    for path in glob::glob(&full)
        .with_context(|| format!("invalid glob pattern for workspace member `{entry}`"))?
    {
        // An unreadable directory is an error, not a shorter member list.
        let path = path.with_context(|| format!("expanding workspace member `{entry}`"))?;
        if path.is_dir() {
            dirs.push(path);
        }
    }
    dirs.sort();
    Ok(dirs)
}

/// Whether the directory `rel` (relative to the workspace root) is named by `entry`.
pub fn member_matches(entry: &str, rel: &Path) -> bool {
    let rel: PathBuf = normalized(&rel.to_string_lossy());
    if is_pattern(entry) {
        let options = glob::MatchOptions {
            case_sensitive: true,
            require_literal_separator: true,
            require_literal_leading_dot: false,
        };
        return glob::Pattern::new(entry.trim_start_matches("./").trim_end_matches('/'))
            .is_ok_and(|p| p.matches_path_with(&rel, options));
    }
    normalized(entry) == rel
}

/// Cargo's `is_excluded`: under an `exclude` path and not under a literal `members` path.
pub fn excluded(exclude: &[String], members: &[String], rel: &Path) -> bool {
    let rel: PathBuf = normalized(&rel.to_string_lossy());
    let under = |entry: &String| rel.starts_with(normalized(entry));
    exclude.iter().any(under) && !members.iter().filter(|m| !is_pattern(m)).any(under)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn patterns_match_like_cargos_glob() {
        let rel = Path::new;
        assert!(member_matches(
            "vendor/*/fixtures",
            rel("vendor/a/fixtures")
        ));
        assert!(!member_matches(
            "vendor/*/fixtures",
            rel("vendor/a/b/fixtures")
        ));
        assert!(member_matches("crates/l?gacy", rel("crates/legacy")));
        assert!(member_matches("**/fixtures", rel("a/b/fixtures")));
        assert!(member_matches("crates/[a-c]x", rel("crates/bx")));
        assert!(member_matches("crates/[!a-c]x", rel("crates/dx")));
        // `^` is a literal in a class, not a negation, as in the `glob` crate.
        assert!(!member_matches("crates/[^a]", rel("crates/b")));
        assert!(member_matches("./tools/", rel("tools")));
        // A failed class match is a failed match: no other matcher steps in.
        assert!(!member_matches("crates/*[ab]", rel("crates/widget[ab]")));
        assert!(validate("crates/[abc").is_err());
        assert!(validate("crates/*").is_ok());
    }

    #[test]
    fn exclude_is_a_path_prefix_and_literal_members_win() {
        let ex = vec!["vendor".to_string()];
        assert!(excluded(&ex, &[], Path::new("vendor/a/fixtures")));
        assert!(!excluded(&ex, &[], Path::new("vendors/a")));
        // Not a pattern: `crates/legacy-*` names no directory.
        assert!(!excluded(
            &["crates/legacy-*".to_string()],
            &[],
            Path::new("crates/legacy-one")
        ));
        let members = vec!["vendor/a/fixtures".to_string()];
        assert!(!excluded(&ex, &members, Path::new("vendor/a/fixtures")));
        // A pattern member does not override an exclusion.
        assert!(excluded(
            &ex,
            &["vendor/*".to_string()],
            Path::new("vendor/b")
        ));
    }

    #[test]
    fn expansion_follows_the_file_system() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        for dir in [
            "crates/a",
            "crates/.hidden",
            "deep/1/2/3/4/5/6/7/8/9/10/11/12/13/14/15/16/fixtures",
        ] {
            fs::create_dir_all(root.join(dir)).unwrap();
        }
        fs::write(root.join("crates/file"), "").unwrap();
        let names = |entry: &str| -> Vec<String> {
            expand(root, entry)
                .unwrap()
                .iter()
                .map(|p| p.strip_prefix(root).unwrap().to_string_lossy().into_owned())
                .collect()
        };
        assert_eq!(names("crates/*"), ["crates/.hidden", "crates/a"]);
        // Glob characters in the root's own name are literal.
        let odd = root.join("project[dev]");
        fs::create_dir_all(odd.join("crates/x")).unwrap();
        let found = expand(&odd, "crates/*").unwrap();
        assert_eq!(found, [odd.join("crates/x")]);
        assert_eq!(
            names("**/fixtures"),
            ["deep/1/2/3/4/5/6/7/8/9/10/11/12/13/14/15/16/fixtures"]
        );
    }
}
