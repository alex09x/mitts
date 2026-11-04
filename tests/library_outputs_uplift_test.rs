/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Cargo places the outputs of a workspace library, `lib<name>.a`, `lib<name>.so`/`.dylib` and
//! `lib<name>.rlib`, in `target/<profile>/`. Mitts placed only the static archive there and kept
//! the dynamic library and the rlib in `deps/`, so a consumer that runs a generator on
//! `target/release/libname.dylib` failed with ENOENT in a fresh checkout (issue 47). The files
//! must be there after a cold build and after a build restored from the cache.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;

const DYLIB: &str = if cfg!(target_os = "macos") {
    "dylib"
} else {
    "so"
};

fn write(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

fn project(dir: &Path, crate_types: &str) -> PathBuf {
    let root = dir.join("lib-project");
    write(
        &root.join("Cargo.toml"),
        &format!(
            "[package]\nname = \"core-demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
             [lib]\nname = \"core_demo\"\ncrate-type = [{crate_types}]\n\n\
             [profile.release]\nopt-level = 3\nlto = true\n"
        ),
    );
    write(&root.join("src/lib.rs"), "pub fn f() -> u32 { 1 }\n");
    root
}

fn build(root: &Path, cache: &Path, release: bool) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mitts"));
    command
        .current_dir(root)
        .arg("build")
        .arg("--cache-dir")
        .arg(cache)
        .env("CARGO_HOME", cache.join("cargo-home"));
    if release {
        command.arg("--release");
    }
    command.output().unwrap()
}

fn expected(root: &Path, profile: &str) -> Vec<PathBuf> {
    let dir = root.join("target").join(profile);
    vec![
        dir.join("libcore_demo.a"),
        dir.join(format!("libcore_demo.{DYLIB}")),
        dir.join("libcore_demo.rlib"),
    ]
}

fn assert_present(files: &[PathBuf], context: &str) {
    for file in files {
        assert!(
            file.is_file() && fs::metadata(file).unwrap().len() > 0,
            "{context}: {} is missing",
            file.display()
        );
    }
}

fn check(release: bool) {
    let dir = TempDir::new().unwrap();
    let cache = dir.path().join("cache");
    let root = project(dir.path(), "\"staticlib\", \"cdylib\", \"rlib\"");
    let profile = if release { "release" } else { "debug" };

    let cold = build(&root, &cache, release);
    assert!(
        cold.status.success(),
        "{}",
        String::from_utf8_lossy(&cold.stderr)
    );
    assert_present(&expected(&root, profile), "cold build");

    // A fresh checkout against the same cache: the outputs are restored, not compiled.
    fs::remove_dir_all(root.join("target")).unwrap();
    let warm = build(&root, &cache, release);
    assert!(
        warm.status.success(),
        "{}",
        String::from_utf8_lossy(&warm.stderr)
    );
    assert_present(&expected(&root, profile), "build restored from the cache");
}

#[test]
fn dev_build_places_all_declared_library_outputs_in_the_profile_directory() {
    check(false);
}

#[test]
fn release_build_places_all_declared_library_outputs_in_the_profile_directory() {
    check(true);
}

#[test]
fn a_plain_library_gets_its_rlib_in_the_profile_directory() {
    let dir = TempDir::new().unwrap();
    let cache = dir.path().join("cache");
    let root = project(dir.path(), "\"lib\"");
    let output = build(&root, &cache, false);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_present(
        &[root.join("target/debug/libcore_demo.rlib")],
        "plain library",
    );
}
