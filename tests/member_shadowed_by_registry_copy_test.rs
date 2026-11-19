/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! itoa 1.0.19's dev-dependencies pull its own published 1.0.19 from crates.io, so Cargo.lock
//! lists `itoa 1.0.19` twice: the registry copy (with a source) and the workspace member
//! (without). Mitts took the first one as the root, planned no unit of the member and
//! finished the build "successfully" with nothing compiled.

use mitts::adapter::rust::types::RustBuildOptions;
use mitts::adapter::RustAdapter;
use std::fs;
use tempfile::TempDir;

#[test]
fn a_member_is_planned_when_a_registry_copy_has_its_name_and_version() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("itoa");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"itoa\"\nversion = \"1.0.19\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(root.join("src/lib.rs"), "pub fn f() {}\n").unwrap();
    // The registry copy comes first, as Mitts writes it.
    fs::write(
        root.join("Cargo.lock"),
        "version = 3\n\
         [[package]]\nname = \"itoa\"\nversion = \"1.0.19\"\n\
         source = \"registry+https://github.com/rust-lang/crates.io-index\"\n\
         [[package]]\nname = \"itoa\"\nversion = \"1.0.19\"\n",
    )
    .unwrap();
    let opt = RustBuildOptions {
        manifest_path: Some(root.join("Cargo.toml")),
        cache_dir: Some(temp.path().join("cache")),
        ..Default::default()
    };
    let plan = RustAdapter::explain_plan(&opt).unwrap();
    assert_eq!(plan.units.len(), 1, "{:?}", plan.units);
    assert!(plan.units[0].is_workspace && plan.units[0].is_primary);
    assert_eq!(plan.units[0].package_id, "itoa@1.0.19");
}
