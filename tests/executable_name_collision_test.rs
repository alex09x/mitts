/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::{RustAdapter, RustBuildOptions};
use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

fn write_binary(root: &Path, name: &str, message: &str) {
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("Cargo.toml"),
        format!(
            "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[workspace]\n"
        ),
    )
    .unwrap();
    fs::write(
        root.join("src/main.rs"),
        format!("fn main() {{ println!(\"{message}\"); }}\n"),
    )
    .unwrap();
}

fn run(binary: &Path) -> String {
    let output = Command::new(binary).output().unwrap();
    assert!(output.status.success(), "{output:?}");
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

#[test]
fn hyphenated_binary_preserves_foreign_underscore_executable_on_publish_and_restore() {
    let workspace = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let foreign_cache = TempDir::new().unwrap();
    let root = workspace.path();
    let cargo_owner = root.join("cargo-owner");
    let target = root.join("target");
    write_binary(root, "foo-bar", "hyphenated");
    write_binary(&cargo_owner, "foo_bar", "foreign underscore");

    let foreign_build = Command::new(env!("CARGO_BIN_EXE_mitts"))
        .args(["build", "--offline", "--manifest-path"])
        .arg(cargo_owner.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(&target)
        .arg("--cache-dir")
        .arg(foreign_cache.path())
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .env_remove("RUSTFLAGS")
        .env_remove("CARGO_MAKEFLAGS")
        .env_remove("MAKEFLAGS")
        .output()
        .unwrap();
    assert!(foreign_build.status.success(), "{foreign_build:?}");
    let foreign = target.join("debug/foo_bar");
    let original = fs::read(&foreign).unwrap();
    assert_eq!(run(&foreign), "foreign underscore");

    let options = RustBuildOptions {
        manifest_path: Some(root.join("Cargo.toml")),
        cache_dir: Some(cache.path().to_path_buf()),
        target_dir: Some(target.clone()),
        native: true,
        // Force the warm build through action-cache restoration, not the no-op lock.
        fast_lock: Some(false),
        ..Default::default()
    };
    let hyphenated = target.join("debug/foo-bar");
    for cached in [false, true] {
        if cached {
            fs::remove_file(&hyphenated).unwrap();
        }
        let (_, result) = RustAdapter::build(&options).expect("build hyphenated binary");
        assert!(result.success, "{}", result.stderr);
        assert_eq!(result.cached, cached, "unexpected cache state");
        assert_eq!(run(&hyphenated), "hyphenated");
        assert_eq!(fs::read(&foreign).unwrap(), original);
        assert_eq!(run(&foreign), "foreign underscore");
    }
}
