/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::rust::lockfile::CargoLock;
use mitts::adapter::rust::manifest::CargoToml;
use mitts::adapter::rust::resolver::{extract_root_package, select_best_version};
use mitts::adapter::rust::sparse_index::SparseIndexEntry;
use std::collections::HashMap;
use std::fs;
use tempfile::TempDir;

#[test]
fn test_m8_lockfile_validation_and_offline_sync() {
    let temp = TempDir::new().unwrap();
    let manifest = temp.path().join("Cargo.toml");
    let lock_path = temp.path().join("Cargo.lock");

    fs::write(
        &manifest,
        r#"
[package]
name = "my-pkg"
version = "0.1.0"

[dependencies]
dep-a = "1.0.0"
dep-b = { version = "2.0.0" }

[target.'cfg(unix)'.dependencies]
unix-dep = "0.5.0"
"#,
    )
    .unwrap();

    // 1. Compatibility with missing manifest file returns Ok(false)
    let non_existent_manifest = temp.path().join("NonExistent.toml");
    let empty_lock = CargoLock::parse("version = 3\n").unwrap();
    assert_eq!(
        empty_lock
            .validate_manifest_compatibility(&non_existent_manifest)
            .unwrap(),
        false
    );

    // 2. Compatibility check when dep is missing from lock returns Ok(false)
    let incomplete_lock = CargoLock::parse(
        r#"
version = 3

[[package]]
name = "my-pkg"
version = "0.1.0"
dependencies = [
    "dep-a",
]

[[package]]
name = "dep-a"
version = "1.0.0"
"#,
    )
    .unwrap();
    assert_eq!(
        incomplete_lock
            .validate_manifest_compatibility(&manifest)
            .unwrap(),
        false
    );

    // 3. Single-line inline dependencies array and unknown keys
    let inline_deps_lock = CargoLock::parse(
        r#"
version = 3

[[package]]
name = "dep-b"
version = "2.0.0"
unknown_key = "some_value"
dependencies = ["dep-a", "unix-dep"]

[[package]]
name = "dep-a"
version = "1.0.0"

[[package]]
name = "unix-dep"
version = "0.5.0"
"#,
    )
    .unwrap();
    let b_pkg = inline_deps_lock.get_package("dep-b").unwrap();
    assert_eq!(b_pkg.dependencies, vec!["dep-a", "unix-dep"]);

    // 4. resolve_or_sync offline failure when lock missing or incompatible
    let res = CargoLock::resolve_or_sync(&manifest, true);
    assert!(res.is_err());
    assert!(res
        .unwrap_err()
        .to_string()
        .contains("Cargo.lock is missing or incompatible and --offline was specified"));

    // 5. resolve_or_sync succeeds when valid compatible lockfile already exists
    let valid_lock = format!(
        r#"version = 3

[[package]]
name = "my-pkg"
version = "0.1.0"

[[package]]
name = "dep-a"
version = "1.0.0"

[[package]]
name = "dep-b"
version = "2.0.0"

[[package]]
name = "unix-dep"
version = "0.5.0"
"#
    );
    fs::write(&lock_path, valid_lock).unwrap();
    let synced = CargoLock::resolve_or_sync(&manifest, true).unwrap();
    assert_eq!(synced.packages.len(), 4);
}

#[test]
fn test_m8_resolver_best_version_and_root_package_extraction() {
    let entries = vec![
        SparseIndexEntry {
            name: "test-pkg".to_string(),
            vers: "1.0.0".to_string(),
            deps: Vec::new(),
            cksum: "hash1".to_string(),
            yanked: false,
            features: HashMap::new(),
        },
        SparseIndexEntry {
            name: "test-pkg".to_string(),
            vers: "1.1.0".to_string(),
            deps: Vec::new(),
            cksum: "hash2".to_string(),
            yanked: true, // Yanked version
            features: HashMap::new(),
        },
        SparseIndexEntry {
            name: "test-pkg".to_string(),
            vers: "1.2.0".to_string(),
            deps: Vec::new(),
            cksum: "hash3".to_string(),
            yanked: false,
            features: HashMap::new(),
        },
    ];

    // Wildcard requirement
    let star = select_best_version(&entries, "*").unwrap();
    assert_eq!(star.vers, "1.2.0");

    let empty = select_best_version(&entries, "").unwrap();
    assert_eq!(empty.vers, "1.2.0");

    // Specific matching requirement
    let match_1_0 = select_best_version(&entries, "=1.0.0").unwrap();
    assert_eq!(match_1_0.vers, "1.0.0");

    // Non-semver or unsatisfied requirement falls back to latest non-yanked
    let fallback = select_best_version(&entries, "invalid_semver_xyz").unwrap();
    assert_eq!(fallback.vers, "1.2.0");

    // All yanked versions fails
    let all_yanked = vec![SparseIndexEntry {
        name: "test-pkg".to_string(),
        vers: "2.0.0".to_string(),
        deps: Vec::new(),
        cksum: "h".to_string(),
        yanked: true,
        features: HashMap::new(),
    }];
    let err = select_best_version(&all_yanked, "^2.0").unwrap_err();
    assert!(err.to_string().contains("No compatible non-yanked version found"));

    // Extract root package
    let toml = "[package]\nname = \"extracted-root\"\nversion = \"0.9.1\"\n";
    let (name, ver) = extract_root_package(toml).unwrap();
    assert_eq!(name, "extracted-root");
    assert_eq!(ver, "0.9.1");
}

#[test]
fn test_m8_manifest_build_script_and_workspace_patterns() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    // 1. Build script variations
    let toml_disabled_false = CargoToml::parse(
        "[package]\nname = \"t1\"\nversion = \"0.1.0\"\nbuild = false\n",
    )
    .unwrap();
    assert!(!toml_disabled_false.has_build_script(root));

    let toml_disabled_true = CargoToml::parse(
        "[package]\nname = \"t2\"\nversion = \"0.1.0\"\nbuild = true\n",
    )
    .unwrap();
    assert!(!toml_disabled_true.has_build_script(root));
    fs::write(root.join("build.rs"), "fn main() {}\n").unwrap();
    assert!(toml_disabled_true.has_build_script(root));

    let toml_custom_path = CargoToml::parse(
        "[package]\nname = \"t3\"\nversion = \"0.1.0\"\nbuild = \"custom_build.rs\"\n",
    )
    .unwrap();
    assert!(!toml_custom_path.has_build_script(root));
    fs::write(root.join("custom_build.rs"), "fn main() {}\n").unwrap();
    assert!(toml_custom_path.has_build_script(root));

    // 2. Discover targets: bin without path falling back to src/bin/<name>.rs
    let src_bin = root.join("src").join("bin");
    fs::create_dir_all(&src_bin).unwrap();
    fs::write(src_bin.join("worker.rs"), "fn main() {}\n").unwrap();

    let toml_bins = CargoToml::parse(
        r#"
[package]
name = "bin-discover"
version = "0.1.0"

[[bin]]
name = "worker"
"#,
    )
    .unwrap();
    let discovered = toml_bins.discover_targets(root);
    assert!(discovered.iter().any(|t| t.name == "worker" && t.kind == "bin"));

    // 3. Workspace members glob "crates/*" and pattern "."
    let ws_toml = CargoToml::parse(
        r#"
[package]
name = "ws-root"
version = "0.1.0"

[workspace]
members = [".", "crates/*", "extra/nested"]
"#,
    )
    .unwrap();

    let crates_dir = root.join("crates").join("child1");
    fs::create_dir_all(&crates_dir).unwrap();
    fs::write(
        crates_dir.join("Cargo.toml"),
        "[package]\nname = \"child1\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();

    let nested_dir = root.join("extra").join("nested");
    fs::create_dir_all(&nested_dir).unwrap();
    fs::write(
        nested_dir.join("Cargo.toml"),
        "[package]\nname = \"nested-pkg\"\nversion = \"0.2.0\"\n",
    )
    .unwrap();

    let members = ws_toml.discover_workspace_members(root);
    assert!(members.iter().any(|(n, _)| n == "ws-root"));
    assert!(members.iter().any(|(n, _)| n == "child1"));
    assert!(members.iter().any(|(n, _)| n == "nested-pkg"));
}
