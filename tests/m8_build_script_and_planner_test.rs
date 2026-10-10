/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::rust::build_script::execute_build_script;
use mitts::adapter::rust::types::PlannedCompilationUnit;
use mitts::adapter::{RustAdapter, RustBuildOptions};
use std::fs;

#[test]
fn test_m8_build_script_with_extern_crates_and_cas_artifacts() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    // Compile a dummy helper rlib to pass as extern_crate
    let helper_src = root.join("helper.rs");
    fs::write(&helper_src, "pub fn helper_val() -> i32 { 10 }\n").unwrap();
    let helper_rlib = root.join("libhelper.rlib");
    let status = std::process::Command::new("rustc")
        .arg("--crate-type=rlib")
        .arg("--crate-name=helper")
        .arg(&helper_src)
        .arg("-o")
        .arg(&helper_rlib)
        .status()
        .unwrap();
    assert!(status.success());

    // Build script that uses the extern crate helper
    let manifest = root.join("Cargo.toml");
    fs::write(
        &manifest,
        r#"
[package]
name = "script-with-extern"
version = "0.1.0"
edition = "2021"

[build-dependencies]
helper = "0.1"
"#,
    )
    .unwrap();

    let build_rs = root.join("build.rs");
    fs::write(
        &build_rs,
        r#"
extern crate helper;
fn main() {
    let _ = helper::helper_val();
    println!("cargo:rustc-cfg=has_helper");
}
"#,
    )
    .unwrap();

    let unit = PlannedCompilationUnit {
        package_id: "script-with-extern@0.1.0".into(),
        package_name: "script-with-extern".into(),
        version: "0.1.0".into(),
        target_name: "script_with_extern".into(),
        kind: "lib".into(),
        src_path: root.join("src").join("lib.rs"),
        manifest_path: manifest,
        cache_eligibility: "Workspace".into(),
        is_workspace: true,
        is_primary: false,
        env_vars: Vec::new(),
        dependencies: Vec::new(),
        edition: Some("2021".into()),
        features: Vec::new(),
        for_host: false,
    };

    let staging = root.join("staging");
    fs::create_dir_all(&staging).unwrap();

    let externs = vec![("helper".to_string(), helper_rlib)];
    let res = execute_build_script(&unit, &staging, false, &[], None, &externs, None);
    assert!(
        res.is_ok(),
        "build script with extern should succeed: {:?}",
        res.err()
    );
    let directives = res.unwrap();
    assert!(directives.cfgs.contains(&"has_helper".to_string()));
}

#[test]
fn test_m8_planner_staticlib_and_untracked_dynamic_env() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    // 1. Staticlib target crate type
    let manifest = root.join("Cargo.toml");
    fs::write(
        &manifest,
        r#"
[package]
name = "static-lib-pkg"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["staticlib"]
path = "src/lib.rs"
"#,
    )
    .unwrap();
    fs::create_dir_all(root.join("src")).unwrap();

    // Include a dynamic untracked env macro call (env!(concat!(...)))
    fs::write(
        root.join("src").join("lib.rs"),
        r#"
pub fn dynamic_env() -> &'static str {
    env!(concat!("MY_", "VAR"))
}
"#,
    )
    .unwrap();

    let opt = RustBuildOptions {
        manifest_path: Some(manifest),
        ..Default::default()
    };

    let explanation = RustAdapter::explain_plan(&opt).unwrap();
    assert_eq!(explanation.units.len(), 1);
    let u = &explanation.units[0];
    assert!(
        u.cache_eligibility.contains("Uncached"),
        "should mark uncached: {}",
        u.cache_eligibility
    );
}
