/*
 * Mitts Build — locate the project manifest the way Cargo does
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::{Path, PathBuf};

/// The manifest a command works on: `--manifest-path` when given, otherwise, like Cargo,
/// the nearest `Cargo.toml` in the current directory or one of its parents.
///
/// `None` keeps the default `./Cargo.toml`, so a missing manifest still reports the
/// current directory.
pub fn discover_manifest(explicit: &Option<PathBuf>) -> Option<PathBuf> {
    if explicit.is_some() {
        return explicit.clone();
    }
    let cwd = std::env::current_dir().ok()?;
    nearest_manifest(&cwd)
}

fn nearest_manifest(start: &Path) -> Option<PathBuf> {
    if start.join("Cargo.toml").is_file() {
        return None;
    }
    start
        .ancestors()
        .skip(1)
        .map(|dir| dir.join("Cargo.toml"))
        .find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn climbs_to_the_nearest_parent_manifest() {
        let temp = tempfile::TempDir::new().unwrap();
        let member = temp.path().join("member");
        std::fs::create_dir_all(member.join("src/nested")).unwrap();
        std::fs::write(temp.path().join("Cargo.toml"), "[workspace]\n").unwrap();
        std::fs::write(member.join("Cargo.toml"), "[package]\n").unwrap();

        assert_eq!(
            nearest_manifest(&member.join("src/nested")),
            Some(member.join("Cargo.toml"))
        );
        assert_eq!(nearest_manifest(&member), None);
        assert_eq!(
            discover_manifest(&Some(PathBuf::from("x/Cargo.toml"))),
            Some(PathBuf::from("x/Cargo.toml"))
        );
    }
}
