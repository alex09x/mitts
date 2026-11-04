/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Binary targets of a package: the `[[bin]]` tables and the ones Cargo finds by itself.
//!
//! Cargo finds `src/main.rs` (named after the package), `src/bin/*.rs` and `src/bin/*/main.rs`
//! unless the package sets `autobins = false`, and it does so also when `[[bin]]` tables exist:
//! a table replaces the found target of the same name or path (to add `required-features`, a
//! different `test` setting, ...) and the others remain.

use super::{CargoToml, DiscoveredTarget};
use std::fs;
use std::path::{Path, PathBuf};

pub(super) fn discover(
    manifest: &CargoToml,
    pkg_dir: &Path,
    edition: Option<&str>,
) -> Vec<DiscoveredTarget> {
    let pkg_name = manifest
        .package
        .as_ref()
        .map(|p| p.name.as_str())
        .unwrap_or("");
    let mut targets = explicit(manifest, pkg_dir, edition);

    let autobins = manifest
        .package
        .as_ref()
        .and_then(|p| p.autobins)
        .unwrap_or(true);
    if !autobins {
        return targets;
    }
    for (name, src_path) in inferred(pkg_dir, pkg_name) {
        let replaced_by_a_table = targets
            .iter()
            .any(|t| t.name == name || same_path(&t.src_path, &src_path));
        if !replaced_by_a_table {
            targets.push(target(name, src_path, edition, None, true));
        }
    }
    targets
}

fn explicit(manifest: &CargoToml, pkg_dir: &Path, edition: Option<&str>) -> Vec<DiscoveredTarget> {
    let mut targets = Vec::new();
    for b in &manifest.bin {
        let src = match b.path {
            Some(ref p) => pkg_dir.join(p),
            None => {
                let candidate1 = pkg_dir
                    .join("src")
                    .join("bin")
                    .join(format!("{}.rs", b.name));
                let candidate2 = pkg_dir
                    .join("src")
                    .join("bin")
                    .join(&b.name)
                    .join("main.rs");
                if candidate1.is_file() {
                    candidate1
                } else if candidate2.is_file() {
                    // Cargo's second inferred location for a named binary.
                    candidate2
                } else {
                    pkg_dir.join("src").join("main.rs")
                }
            }
        };
        if src.is_file() {
            targets.push(target(
                b.name.clone(),
                src,
                edition,
                b.required_features.clone(),
                b.harness.unwrap_or(true),
            ));
        }
    }
    targets
}

/// The binaries Cargo infers from the layout, in a stable order: `src/main.rs`, then
/// `src/bin/*` by name.
fn inferred(pkg_dir: &Path, pkg_name: &str) -> Vec<(String, PathBuf)> {
    let mut found = Vec::new();
    let default_main = pkg_dir.join("src").join("main.rs");
    if default_main.is_file() && !pkg_name.is_empty() {
        found.push((pkg_name.to_string(), default_main));
    }
    let mut in_bin_dir = Vec::new();
    if let Ok(entries) = fs::read_dir(pkg_dir.join("src").join("bin")) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("rs") {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    in_bin_dir.push((stem.to_string(), path));
                }
            } else if path.is_dir() {
                let main_rs = path.join("main.rs");
                if main_rs.is_file() {
                    if let Some(dir_name) = path.file_name().and_then(|s| s.to_str()) {
                        in_bin_dir.push((dir_name.to_string(), main_rs));
                    }
                }
            }
        }
    }
    in_bin_dir.sort();
    found.extend(in_bin_dir);
    found
}

fn target(
    name: String,
    src_path: PathBuf,
    edition: Option<&str>,
    required_features: Option<Vec<String>>,
    harness: bool,
) -> DiscoveredTarget {
    DiscoveredTarget {
        name,
        kind: "bin".to_string(),
        src_path,
        edition: edition.map(str::to_owned),
        crate_types: vec!["bin".to_string()],
        required_features,
        harness,
        test_role: None,
        explicit_only: false,
    }
}

/// Paths compared by their components, so `./src/bin/a.rs` and `src/bin/a.rs` are one file.
fn same_path(a: &Path, b: &Path) -> bool {
    a.components().eq(b.components())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(manifest: &str, files: &[&str]) -> (tempfile::TempDir, Vec<DiscoveredTarget>) {
        let dir = tempfile::tempdir().unwrap();
        for file in files {
            let path = dir.path().join(file);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, "fn main() {}\n").unwrap();
        }
        let manifest: CargoToml = toml::from_str(manifest).unwrap();
        let targets = discover(&manifest, dir.path(), Some("2021"));
        (dir, targets)
    }

    fn names(targets: &[DiscoveredTarget]) -> Vec<&str> {
        targets.iter().map(|t| t.name.as_str()).collect()
    }

    const PACKAGE: &str = "[package]\nname = \"pkg\"\nversion = \"0.1.0\"\n";

    #[test]
    fn a_table_does_not_hide_the_inferred_binaries() {
        let manifest =
            format!("{PACKAGE}\n[[bin]]\nname = \"explicit\"\npath = \"src/bin/explicit.rs\"\n");
        let (_dir, targets) = layout(
            &manifest,
            &[
                "src/bin/explicit.rs",
                "src/bin/auto.rs",
                "src/bin/dir/main.rs",
            ],
        );
        assert_eq!(names(&targets), ["explicit", "auto", "dir"]);
    }

    #[test]
    fn src_main_stays_next_to_explicit_tables() {
        let manifest = format!("{PACKAGE}\n[[bin]]\nname = \"other\"\npath = \"src/other.rs\"\n");
        let (_dir, targets) = layout(&manifest, &["src/main.rs", "src/other.rs"]);
        assert_eq!(names(&targets), ["other", "pkg"]);
    }

    #[test]
    fn a_table_replaces_the_inferred_target_of_the_same_name_or_path() {
        let by_name = format!("{PACKAGE}\n[[bin]]\nname = \"auto\"\nrequired-features = [\"x\"]\n");
        let (_dir, targets) = layout(&by_name, &["src/bin/auto.rs"]);
        assert_eq!(names(&targets), ["auto"]);
        assert_eq!(targets[0].required_features, Some(vec!["x".to_string()]));

        let by_path =
            format!("{PACKAGE}\n[[bin]]\nname = \"renamed\"\npath = \"./src/bin/auto.rs\"\n");
        let (_dir, targets) = layout(&by_path, &["src/bin/auto.rs"]);
        assert_eq!(names(&targets), ["renamed"]);

        let main_by_name = format!("{PACKAGE}\n[[bin]]\nname = \"pkg\"\n");
        let (_dir, targets) = layout(&main_by_name, &["src/main.rs"]);
        assert_eq!(names(&targets), ["pkg"]);
    }

    #[test]
    fn autobins_false_leaves_only_the_tables() {
        let manifest = "[package]\nname = \"pkg\"\nversion = \"0.1.0\"\nautobins = false\n\n\
                        [[bin]]\nname = \"explicit\"\npath = \"src/bin/explicit.rs\"\n";
        let (_dir, targets) = layout(manifest, &["src/bin/explicit.rs", "src/bin/auto.rs"]);
        assert_eq!(names(&targets), ["explicit"]);
    }

    #[test]
    fn without_tables_the_layout_decides_in_a_stable_order() {
        let (_dir, targets) = layout(
            PACKAGE,
            &[
                "src/main.rs",
                "src/bin/zeta.rs",
                "src/bin/alpha.rs",
                "src/bin/mid/main.rs",
            ],
        );
        assert_eq!(names(&targets), ["pkg", "alpha", "mid", "zeta"]);
    }
}
