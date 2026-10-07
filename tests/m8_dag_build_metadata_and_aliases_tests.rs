/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::rust::types::RustBuildOptions;
use mitts::adapter::RustAdapter;
use std::fs;
use tempfile::TempDir;

#[test]
fn test_m8_dag_build_metadata_propagation_and_aliases() {
    let temp_dir = TempDir::new().expect("tempdir");
    let ws_dir = temp_dir.path().join("ws");
    let sys_a_dir = ws_dir.join("sys_a");
    let dep_b_dir = ws_dir.join("dep_b");
    fs::create_dir_all(&sys_a_dir).unwrap();
    fs::create_dir_all(&dep_b_dir).unwrap();

    // 1. Workspace root manifest
    fs::write(
        ws_dir.join("Cargo.toml"),
        "[workspace]\nmembers = [\"sys_a\", \"dep_b\"]\n",
    )
    .unwrap();

    // 2. sys_a crate with links = "sys-a" and build.rs emitting metadata
    fs::write(
        sys_a_dir.join("Cargo.toml"),
        r#"[package]
name = "sys-a"
version = "0.1.0"
links = "sys-a"
"#,
    )
    .unwrap();
    fs::write(
        sys_a_dir.join("build.rs"),
        r#"fn main() {
    println!("cargo:include_dir=/usr/include/sys_a");
    println!("cargo:rustc-cfg=sys_a_ready");
}
"#,
    )
    .unwrap();
    let sys_a_src = sys_a_dir.join("src");
    fs::create_dir_all(&sys_a_src).unwrap();
    fs::write(sys_a_src.join("lib.rs"), "pub fn sys_a() {}\n").unwrap();

    // 3. dep_b crate with renamed dependency alias
    fs::write(
        dep_b_dir.join("Cargo.toml"),
        r#"[package]
name = "dep-b"
version = "0.1.0"

[dependencies]
aliased_sys = { package = "sys-a", path = "../sys_a" }
"#,
    )
    .unwrap();
    let dep_b_src = dep_b_dir.join("src");
    fs::create_dir_all(&dep_b_src).unwrap();
    fs::write(
        dep_b_src.join("lib.rs"),
        r#"extern crate aliased_sys;
pub fn dep_b() {
    aliased_sys::sys_a();
}
"#,
    )
    .unwrap();

    // 4. Build workspace with native DAG runner
    let opt = RustBuildOptions {
        manifest_path: Some(ws_dir.join("Cargo.toml")),
        target_dir: Some(temp_dir.path().join("target")),
        cache_dir: Some(temp_dir.path().join("cache")),
        native: true,
        ..Default::default()
    };

    let (plan, res) = RustAdapter::build(&opt).unwrap();
    assert!(res.success, "Build should succeed with metadata propagation and aliases: {}", res.stderr);
    assert!(plan.actions.len() >= 2);
}
