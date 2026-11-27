/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A compile error of a workspace member shows its file, line and source snippet in the
//! human output (#195): rustc runs in the workspace root and names `app/src/...`, and the
//! report looked the file up from the package directory, so it printed only the message.

use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

fn workspace() -> TempDir {
    let dir = TempDir::new().unwrap();
    let root = dir.path().join("ws");
    fs::create_dir_all(root.join("app/src")).unwrap();
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"app\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    fs::write(
        root.join("app/Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(
        root.join("app/src/lib.rs"),
        "pub fn f() -> u32 { 1 }\n#[cfg(test)]\nmod route_tests;\n",
    )
    .unwrap();
    fs::write(
        root.join("app/src/route_tests.rs"),
        "#[test]\nfn t() {\n    let v: u32 = \"not a number\";\n    assert_eq!(v, 1);\n}\n",
    )
    .unwrap();
    dir
}

fn failed_stderr(dir: &Path, cwd: &Path, args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_mitts"))
        .args(args)
        .arg("--cache-dir")
        .arg(dir.join("cache"))
        .current_dir(cwd)
        .env("CARGO_HOME", dir.join("cargo-home"))
        .env("NO_COLOR", "1")
        .env_remove("MITTS_CACHE_DIR")
        .env_remove("RUSTC_WRAPPER")
        .env_remove("CARGO_BUILD_RUSTC_WRAPPER")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        !out.status.success(),
        "the error fails the build:\n{stderr}"
    );
    stderr
}

#[test]
fn a_members_error_shows_its_file_line_and_snippet() {
    let dir = workspace();
    let root = dir.path().join("ws");
    for (cwd, args) in [
        (root.clone(), vec!["test", "-p", "app", "--lib"]),
        (root.join("app"), vec!["test", "--lib"]),
    ] {
        let stderr = failed_stderr(dir.path(), &cwd, &args);
        assert!(
            stderr.contains("route_tests.rs:3:"),
            "{args:?} in {}: the location\n{stderr}",
            cwd.display()
        );
        assert!(
            stderr.contains("let v: u32 = \"not a number\";"),
            "{args:?} in {}: the snippet\n{stderr}",
            cwd.display()
        );
    }
}
