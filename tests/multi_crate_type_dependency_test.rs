/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A dependency whose manifest says `crate-type = ["lib", "cdylib", "staticlib"]` must still be
//! built as an rlib: `lib` is the Rust library. It was passed to rustc as only `cdylib` and
//! `staticlib`, so the dependent crate got `--extern dep=libdep.so` and failed with
//! "found invalid metadata files ... no `.rustc` section" (issue 46) while `mitts check`, which
//! needs only metadata, passed.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use tempfile::TempDir;

fn write(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

fn project(crate_types: &str) -> TempDir {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    write(
        &root.join("dep/Cargo.toml"),
        &format!(
            "[package]\nname = \"multidep\"\nversion = \"0.2.0\"\nedition = \"2021\"\n\n\
             [lib]\ncrate-type = [{crate_types}]\n"
        ),
    );
    write(
        &root.join("dep/src/lib.rs"),
        "pub fn helper() -> u32 { 41 }\n",
    );
    write(
        &root.join("app/Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
         [dependencies]\nmultidep = { path = \"../dep\" }\n",
    );
    write(
        &root.join("app/src/lib.rs"),
        "pub fn f() -> u32 { multidep::helper() + 1 }\n",
    );
    dir
}

fn mitts(dir: &TempDir, command: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mitts"))
        .current_dir(dir.path().join("app"))
        .arg(command)
        .arg("--cache-dir")
        .arg(dir.path().join("cache"))
        .env("CARGO_HOME", dir.path().join("cargo-home"))
        .output()
        .unwrap()
}

fn assert_builds(crate_types: &str) {
    let dir = project(crate_types);
    for command in ["check", "build"] {
        let output = mitts(&dir, command);
        assert!(
            output.status.success(),
            "mitts {command} with crate-type [{crate_types}]:\n{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn a_dependency_with_lib_cdylib_and_staticlib_builds() {
    assert_builds("\"lib\", \"cdylib\", \"staticlib\"");
}

#[test]
fn a_dependency_with_rlib_and_cdylib_builds() {
    assert_builds("\"rlib\", \"cdylib\"");
}

#[test]
fn a_dependency_with_only_lib_builds() {
    assert_builds("\"lib\"");
}
