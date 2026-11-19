/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! One workspace, checked against `cargo build -p a --unit-graph` (Cargo 1.100 nightly):
//! `shared` gets `["d", "default", "x"]`.
//! - `a` declares `shared` without `workspace = true`, so `[workspace.dependencies]`
//!   (`default-features = false`, `features = ["y"]`) does not apply; Mitts merged it anyway.
//! - member `b`'s dev-dependency on `shared` with `from_b` is not selected, so even resolver 1
//!   leaves `from_b` off; Mitts' first resolver-1 fix walked every member's dev-dependencies.
//! - `c` declares `shared = { workspace = true, default-features = false }`: Cargo ignores a
//!   member turning default features off and keeps the workspace entry's choice.

use mitts::adapter::rust::types::RustBuildOptions;
use mitts::adapter::RustAdapter;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

fn member(root: &Path, name: &str, manifest: &str) {
    fs::create_dir_all(root.join(name).join("src")).unwrap();
    fs::write(root.join(name).join("Cargo.toml"), manifest).unwrap();
    fs::write(root.join(name).join("src/lib.rs"), "pub fn f() {}\n").unwrap();
}

fn shared_features(root: &Path, package: &str, cache: &Path) -> Vec<String> {
    let opt = RustBuildOptions {
        manifest_path: Some(root.join("Cargo.toml")),
        package: Some(package.to_string()),
        cache_dir: Some(cache.to_path_buf()),
        ..Default::default()
    };
    let plan = RustAdapter::explain_plan(&opt).unwrap();
    let mut feats = plan
        .units
        .iter()
        .find(|u| u.package_name == "shared")
        .expect("shared is built")
        .features
        .clone();
    feats.sort();
    feats
}

#[test]
fn workspace_dependencies_and_member_dev_dependencies_match_cargo() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("ws");
    member(
        &root,
        "shared",
        "[package]\nname = \"shared\"\nversion = \"0.1.0\"\n\
         [features]\ndefault = [\"d\"]\nd = []\nfrom_b = []\nx = []\ny = []\n",
    );
    member(
        &root,
        "a",
        "[package]\nname = \"a\"\nversion = \"0.1.0\"\n\
         [dependencies]\nb = { path = \"../b\" }\nshared = { path = \"../shared\", features = [\"x\"] }\n",
    );
    member(
        &root,
        "b",
        "[package]\nname = \"b\"\nversion = \"0.1.0\"\n\
         [dev-dependencies]\nshared = { path = \"../shared\", features = [\"from_b\"] }\n",
    );
    member(
        &root,
        "c",
        "[package]\nname = \"c\"\nversion = \"0.1.0\"\n\
         [dependencies]\nshared = { workspace = true, default-features = false }\n",
    );
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"a\", \"b\", \"c\", \"shared\"]\n\
         [workspace.dependencies]\nshared = { path = \"shared\", features = [\"y\"] }\n",
    )
    .unwrap();
    let cache = temp.path().join("cache");
    assert_eq!(shared_features(&root, "a", &cache), ["d", "default", "x"]);
    assert_eq!(
        shared_features(&root, "c", &cache),
        ["d", "default", "y"],
        "the workspace entry keeps default features on"
    );

    // Edition 2024 lets the member turn them off (Cargo 1.99+).
    member(
        &root,
        "c",
        "[package]\nname = \"c\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\
         [dependencies]\nshared = { workspace = true, default-features = false }\n",
    );
    assert_eq!(shared_features(&root, "c", &cache), ["y"]);
}
