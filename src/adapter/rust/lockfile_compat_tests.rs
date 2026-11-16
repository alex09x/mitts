/*
 * Mitts Build — tests for the Cargo.lock fast path
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Issue 136: the fast path decided the lock was current when only a new
//! `[dev-dependencies]` entry (or a member's, in a workspace) was missing from it, so the
//! test target that used the crate failed with `use of undeclared crate`.

use super::*;

const LOCK: &str = r#"version = 4

[[package]]
name = "app"
version = "0.1.0"

[[package]]
name = "itoa"
version = "1.0.11"
source = "registry+https://github.com/rust-lang/crates.io-index"
"#;

fn manifest(dir: &Path, rel: &str, text: &str) -> std::path::PathBuf {
    let path = dir.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, text).unwrap();
    path
}

const PKG: &str = "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";

fn compatible(manifest_path: &Path) -> bool {
    CargoLock::parse(LOCK)
        .unwrap()
        .validate_manifest_compatibility(manifest_path)
        .unwrap()
}

#[test]
fn a_dev_dependency_missing_from_the_lock_needs_a_resolve() {
    let dir = tempfile::tempdir().unwrap();
    let path = manifest(
        dir.path(),
        "Cargo.toml",
        &format!("{PKG}\n[dev-dependencies]\nitoa = \"1\"\n"),
    );
    assert!(compatible(&path), "itoa 1.0.11 is locked");

    let path = manifest(
        dir.path(),
        "Cargo.toml",
        &format!("{PKG}\n[dev-dependencies]\nitoa = \"1\"\nryu = \"1\"\n"),
    );
    assert!(!compatible(&path), "ryu is not locked");

    // A requirement the locked version does not meet also needs a resolve.
    let path = manifest(
        dir.path(),
        "Cargo.toml",
        &format!("{PKG}\n[dev-dependencies]\nitoa = \"2\"\n"),
    );
    assert!(!compatible(&path));
}

#[test]
fn target_specific_dependencies_of_every_kind_count() {
    let dir = tempfile::tempdir().unwrap();
    for table in ["dependencies", "build-dependencies", "dev-dependencies"] {
        let path = manifest(
            dir.path(),
            "Cargo.toml",
            &format!("{PKG}\n[target.'cfg(windows)'.{table}]\nryu = \"1\"\n"),
        );
        assert!(!compatible(&path), "{table}");
    }
    // Optional and path dependencies are not checked here, as before.
    let path = manifest(
        dir.path(),
        "Cargo.toml",
        &format!("{PKG}\n[dev-dependencies]\nryu = {{ version = \"1\", optional = true }}\nlocal = {{ path = \"local\" }}\n"),
    );
    assert!(compatible(&path));
}

#[test]
fn a_new_dev_dependency_of_a_workspace_member_needs_a_resolve() {
    let dir = tempfile::tempdir().unwrap();
    let root = manifest(
        dir.path(),
        "Cargo.toml",
        "[workspace]\nmembers = [\"app\"]\nresolver = \"2\"\n",
    );
    manifest(
        dir.path(),
        "app/Cargo.toml",
        &format!("{PKG}\n[dev-dependencies]\nitoa = \"1\"\n"),
    );
    manifest(dir.path(), "app/src/lib.rs", "");
    assert!(compatible(&root));

    manifest(
        dir.path(),
        "app/Cargo.toml",
        &format!("{PKG}\n[dev-dependencies]\nitoa = \"1\"\nryu = \"1\"\n"),
    );
    assert!(
        !compatible(&root),
        "the member's new dev-dependency is not locked"
    );
}
