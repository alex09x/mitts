/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A `[[test]]` without `path` lives at `tests/<name>.rs` or `tests/<name>/main.rs`. With
//! `autotests = false` only the explicit entries exist, and Mitts tried just the flat file:
//! toml_edit's `testsuite`, `compliance` and `serde` directory tests were never built.

use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

fn write_project(dir: &Path) {
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::create_dir_all(dir.join("tests/suite")).unwrap();
    fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\nautotests = false\n\n\
         [features]\ndefault = [\"x\"]\nx = []\n\n\
         [[test]]\nname = \"suite\"\nrequired-features = [\"x\"]\n",
    )
    .unwrap();
    fs::write(dir.join("src/lib.rs"), "pub fn v() -> u32 { 7 }\n").unwrap();
    fs::write(
        dir.join("tests/suite/main.rs"),
        "mod part;\n#[test]\nfn suite_root() { assert_eq!(app::v(), 7); }\n",
    )
    .unwrap();
    fs::write(dir.join("tests/suite/part.rs"), "#[test]\nfn suite_part() {}\n").unwrap();
}

#[test]
fn explicit_test_without_path_resolves_to_the_directory_form() {
    let cache = TempDir::new().unwrap();
    let dir = TempDir::new().unwrap();
    write_project(dir.path());
    let out = Command::new(env!("CARGO_BIN_EXE_mitts"))
        .args(["test", "--tests", "--manifest-path"])
        .arg(dir.path().join("Cargo.toml"))
        .arg("--cache-dir")
        .arg(cache.path())
        .env("CI", "true")
        .env_remove("MITTS_CACHE_DIR")
        .output()
        .expect("run mitts");
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "the directory test builds and passes:\n{text}");
    assert!(text.contains("suite_root") && text.contains("suite_part"), "both tests ran:\n{text}");
}
