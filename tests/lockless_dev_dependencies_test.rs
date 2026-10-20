/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A project without a `Cargo.lock` gets one from Mitts' own resolver. Like
//! `cargo generate-lockfile`, that lockfile must carry the package's
//! dev-dependencies and optional dependencies, otherwise `mitts test` cannot
//! build the tests and Cargo rewrites the file on its next run.
//! Needs crates.io (the sparse index and the `itoa` crate).

use mitts::adapter::CargoLock;
use std::fs;
use std::process::Command;
use tempfile::TempDir;

#[test]
fn generated_lockfile_carries_dev_and_optional_dependencies_and_tests_build() {
    let temp = TempDir::new().unwrap();
    let project = temp.path().join("lockless");
    fs::create_dir_all(project.join("src")).unwrap();
    fs::create_dir_all(project.join("tests")).unwrap();
    fs::write(
        project.join("Cargo.toml"),
        "[package]\nname = \"lockless\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n\n[dependencies]\nryu = { version = \"1\", optional = true }\n\n[dev-dependencies]\nitoa = \"1\"\n\n[features]\nfloats = [\"ryu\"]\n",
    )
    .unwrap();
    fs::write(project.join("src/lib.rs"), "pub fn n() -> u8 { 3 }\n").unwrap();
    fs::write(
        project.join("tests/uses_dev_dep.rs"),
        "#[test]\nfn formats() { assert_eq!(itoa::Buffer::new().format(lockless::n()), \"3\"); }\n",
    )
    .unwrap();
    assert!(!project.join("Cargo.lock").exists());

    let cache = temp.path().join("cache");
    let out = Command::new(env!("CARGO_BIN_EXE_mitts"))
        .arg("test")
        .arg("--no-run")
        .arg("--manifest-path")
        .arg(project.join("Cargo.toml"))
        .arg("--cache-dir")
        .arg(&cache)
        .env("CI", "true")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "mitts test --no-run on a lockless project failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let lock = CargoLock::from_file(&project.join("Cargo.lock")).unwrap();
    let root = lock.get_package("lockless").expect("root package in lock");
    assert!(
        root.dependencies.iter().any(|d| d == "itoa"),
        "dev-dependency missing from the generated lockfile: {:?}",
        root.dependencies
    );
    assert!(
        root.dependencies.iter().any(|d| d == "ryu"),
        "optional dependency missing from the generated lockfile: {:?}",
        root.dependencies
    );
    assert!(lock.get_package("itoa").is_some());
    assert!(lock.get_package("ryu").is_some());
}
