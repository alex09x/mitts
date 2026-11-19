/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! `workspace.default-members` takes the same globs as `members`: ratatui lists
//! `examples/apps/*`, and Cargo builds every example app by default while Mitts used to
//! match the pattern as a literal path and plan none of them.

use mitts::adapter::rust::types::RustBuildOptions;
use mitts::adapter::RustAdapter;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

fn write(path: &Path, contents: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn member(root: &Path, rel: &str, name: &str) {
    write(
        &root.join(rel).join("Cargo.toml"),
        &format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
    );
    write(&root.join(rel).join("src/lib.rs"), "pub fn f() {}\n");
}

#[test]
fn a_glob_in_default_members_selects_every_matching_member() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("ws");
    write(
        &root.join("Cargo.toml"),
        "[workspace]\nresolver = \"2\"\nmembers = [\"core\", \"apps/*\", \"tools/*\"]\n\
         default-members = [\"core\", \"apps/*\"]\n",
    );
    member(&root, "core", "core");
    member(&root, "apps/one", "app-one");
    member(&root, "apps/two", "app-two");
    member(&root, "tools/gen", "gen");
    write(
        &root.join("Cargo.lock"),
        "version = 3\n\
         [[package]]\nname = \"app-one\"\nversion = \"0.1.0\"\n\
         [[package]]\nname = \"app-two\"\nversion = \"0.1.0\"\n\
         [[package]]\nname = \"core\"\nversion = \"0.1.0\"\n\
         [[package]]\nname = \"gen\"\nversion = \"0.1.0\"\n",
    );
    let opt = RustBuildOptions {
        manifest_path: Some(root.join("Cargo.toml")),
        cache_dir: Some(temp.path().join("cache")),
        ..Default::default()
    };
    let plan = RustAdapter::explain_plan(&opt).unwrap();
    let mut names: Vec<&str> = plan.units.iter().map(|u| u.package_name.as_str()).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(
        names,
        ["app-one", "app-two", "core"],
        "`tools/*` is not a default member"
    );
}
