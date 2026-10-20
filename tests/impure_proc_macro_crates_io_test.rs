/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A real crates.io macro that reads package files without telling rustc:
//! `include_dir!` embeds a directory found through `$CARGO_MANIFEST_DIR` and
//! registers nothing in dep-info on stable. Declaring the crate under
//! `[workspace.metadata.mitts] impure-proc-macros` puts the embedded files into
//! the dependents' key; without the declaration an edit is invisible to the key,
//! exactly as it is to Cargo. Needs crates.io (the sparse index and `include_dir`).

use mitts::adapter::rust::key::derive_unit_action_keys;
use mitts::adapter::{RustAdapter, RustBuildOptions};
use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

fn write_project(root: &Path, mitts_table: &str, greeting: &str) {
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(root.join("assets")).unwrap();
    fs::write(
        root.join("Cargo.toml"),
        format!(
            "[package]\nname = \"assets_app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n\n[dependencies]\ninclude_dir = \"0.7\"\n{mitts_table}"
        ),
    )
    .unwrap();
    fs::write(
        root.join("src/main.rs"),
        r#"use include_dir::{include_dir, Dir};

static ASSETS: Dir = include_dir!("$CARGO_MANIFEST_DIR/assets");

fn main() {
    let file = ASSETS.get_file("greeting.txt").expect("greeting.txt embedded");
    print!("ASSET:{}", file.contents_utf8().unwrap().trim());
}
"#,
    )
    .unwrap();
    fs::write(root.join("assets/greeting.txt"), greeting).unwrap();
}

fn options(root: &Path, cache: &Path) -> RustBuildOptions {
    RustBuildOptions {
        manifest_path: Some(root.join("Cargo.toml")),
        cache_dir: Some(cache.to_path_buf()),
        target_dir: Some(root.join("target")),
        ..Default::default()
    }
}

fn app_key(opt: &RustBuildOptions) -> String {
    let plan = RustAdapter::explain_plan(opt).expect("explain");
    let keys = derive_unit_action_keys(opt, &plan).expect("keys");
    let app = plan
        .units
        .iter()
        .find(|u| u.package_name == "assets_app" && u.kind == "bin")
        .expect("app unit");
    keys[&app.unit_id()].clone()
}

/// Builds, asserts success, and returns (app restored from cache, program output).
fn build_and_run(root: &Path, opt: &RustBuildOptions) -> (bool, String) {
    let (plan, res) = RustAdapter::build(opt).expect("build");
    assert!(res.success, "build failed:\n{}", res.stderr);
    let restored = plan
        .actions
        .iter()
        .any(|a| a.description.contains("Restored assets_app"));
    let out = Command::new(root.join("target/debug/assets_app"))
        .output()
        .expect("run assets_app");
    (restored, String::from_utf8_lossy(&out.stdout).to_string())
}

#[test]
fn declared_include_dir_puts_the_embedded_files_into_the_key() {
    let cache = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    write_project(
        project.path(),
        "\n[package.metadata.mitts]\nimpure-proc-macros = [\"include_dir\"]\n",
        "hello",
    );
    let opt = options(project.path(), cache.path());

    let (restored, out) = build_and_run(project.path(), &opt);
    assert!(!restored);
    assert_eq!(out, "ASSET:hello");

    let key_before = app_key(&opt);
    fs::write(project.path().join("assets/greeting.txt"), "bonjour").unwrap();
    assert_ne!(
        key_before,
        app_key(&opt),
        "an edited embedded file must change the key of a declared macro's dependent"
    );
    let (restored, out) = build_and_run(project.path(), &opt);
    assert!(!restored, "the edit must rebuild the app");
    assert_eq!(out, "ASSET:bonjour");

    let (restored, out) = build_and_run(project.path(), &opt);
    assert!(restored, "unchanged files must hit the cache");
    assert_eq!(out, "ASSET:bonjour");
}

#[test]
fn undeclared_include_dir_leaves_the_embedded_files_out_of_the_key() {
    let cache = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    write_project(project.path(), "", "hello");
    let opt = options(project.path(), cache.path());

    let (_, out) = build_and_run(project.path(), &opt);
    assert_eq!(out, "ASSET:hello");

    // include_dir reports nothing to rustc, so without the declaration the asset
    // is not an input of the key: this is the case the declaration exists for.
    let key_before = app_key(&opt);
    fs::write(project.path().join("assets/greeting.txt"), "bonjour").unwrap();
    assert_eq!(
        key_before,
        app_key(&opt),
        "without a declaration a silently read file is not part of the key"
    );
}
