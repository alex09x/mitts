/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::rust::manifest::CargoToml;
use mitts::adapter::rust::types::{BuildMode, RustBuildOptions};
use mitts::adapter::RustAdapter;
use std::fs;
use tempfile::TempDir;

#[test]
fn test_m8_planner_cdylib_and_test_mode_planning() {
    let tmp = TempDir::new().unwrap();
    let root_dir = tmp.path();

    // 1. Crate with cdylib crate-type
    let cdylib_toml = root_dir.join("Cargo.toml");
    fs::write(
        &cdylib_toml,
        r#"
[package]
name = "cdylib-crate"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib", "rlib"]
"#,
    )
    .unwrap();

    let src_dir = root_dir.join("src");
    fs::create_dir_all(&src_dir).unwrap();
    fs::write(src_dir.join("lib.rs"), "pub fn c_lib() {}").unwrap();

    let opt = RustBuildOptions {
        manifest_path: Some(cdylib_toml.clone()),
        ..Default::default()
    };

    let plan = RustAdapter::explain_plan(&opt).unwrap();
    assert_eq!(plan.units.len(), 1);
    assert!(plan.units[0]
        .cache_eligibility
        .contains("native/dynamic library"));

    // 2. Test mode planning discovers test targets
    let test_dir = root_dir.join("tests");
    fs::create_dir_all(&test_dir).unwrap();
    fs::write(test_dir.join("integration.rs"), "#[test] fn test_it() {}").unwrap();

    let opt_test = RustBuildOptions {
        manifest_path: Some(cdylib_toml),
        mode: BuildMode::Test,
        ..Default::default()
    };

    let test_plan = RustAdapter::explain_plan(&opt_test).unwrap();
    assert!(test_plan.units.iter().any(|u| u.kind == "test"));
}

#[test]
fn test_m8_planner_untracked_env_macro_eligibility() {
    let tmp = TempDir::new().unwrap();
    let root_dir = tmp.path();

    let toml_path = root_dir.join("Cargo.toml");
    fs::write(
        &toml_path,
        r#"
[package]
name = "dynamic-env-crate"
version = "0.1.0"
edition = "2021"
"#,
    )
    .unwrap();

    let src_dir = root_dir.join("src");
    fs::create_dir_all(&src_dir).unwrap();
    // Non-literal expression inside env! macro (untracked dynamic)
    fs::write(
        src_dir.join("main.rs"),
        r#"
fn main() {
    let x = env!(concat!("FOO", "_BAR"));
}
"#,
    )
    .unwrap();

    let opt = RustBuildOptions {
        manifest_path: Some(toml_path),
        ..Default::default()
    };

    let plan = RustAdapter::explain_plan(&opt).unwrap();
    assert_eq!(plan.units.len(), 1);
    assert!(plan.units[0]
        .cache_eligibility
        .contains("untracked dynamic env!"));
}

#[test]
fn test_m8_manifest_workspace_and_targets() {
    let tmp = TempDir::new().unwrap();
    let root_dir = tmp.path();

    let ws_toml = root_dir.join("Cargo.toml");
    fs::write(
        &ws_toml,
        r#"
[workspace]
members = ["crates/*"]
"#,
    )
    .unwrap();

    let crate_a = root_dir.join("crates").join("pkg_a");
    fs::create_dir_all(crate_a.join("src")).unwrap();
    fs::write(
        crate_a.join("Cargo.toml"),
        "[package]\nname = \"pkg_a\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(crate_a.join("src").join("lib.rs"), "pub fn a() {}").unwrap();

    let toml = CargoToml::from_file(&ws_toml).unwrap();
    let members = toml.discover_workspace_members(root_dir);
    assert_eq!(members.len(), 1);
    assert_eq!(members[0].0, "pkg_a");
}
