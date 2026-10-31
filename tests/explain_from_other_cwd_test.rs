/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! `mitts explain --manifest-path <abs>` run from an unrelated directory failed with
//! "Manifest snapshot has an unknown input: Failed to read manifest <cwd>/./Cargo.toml":
//! the feature index read the `[patch]` tables from `<cwd>/Cargo.toml` instead of the
//! project's root manifest. Cargo resolves `--manifest-path` independently of the cwd.

use std::fs;
use std::process::Command;
use tempfile::TempDir;

#[test]
fn explain_with_a_manifest_path_does_not_depend_on_the_current_directory() {
    let cache = TempDir::new().unwrap();
    let dir = TempDir::new().unwrap();
    let elsewhere = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(dir.path().join("src/lib.rs"), "pub fn v() -> u32 { 1 }\n").unwrap();
    for args in [&["explain"][..], &["check"][..]] {
        let out = Command::new(env!("CARGO_BIN_EXE_mitts"))
            .args(args)
            .arg("--manifest-path")
            .arg(dir.path().join("Cargo.toml"))
            .arg("--cache-dir")
            .arg(cache.path())
            .current_dir(elsewhere.path())
            .env("CI", "true")
            .env_remove("MITTS_CACHE_DIR")
            .output()
            .expect("run mitts");
        assert!(
            out.status.success(),
            "mitts {} from another directory:\n{}\n{}",
            args[0],
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
