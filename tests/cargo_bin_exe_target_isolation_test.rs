/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::tempdir;

fn mitts_bin() -> PathBuf {
    std::env::var("CARGO_BIN_EXE_mitts")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let mut p = std::env::current_exe().expect("current exe");
            p.pop();
            if p.ends_with("deps") {
                p.pop();
            }
            p.join("mitts")
        })
}

fn run_mitts_test(ws: &Path, home: &Path) -> (Output, String) {
    let out = Command::new(mitts_bin())
        .arg("test")
        .current_dir(ws)
        .env("HOME", home)
        .env_remove("MITTS_CACHE_DIR")
        .env_remove("XDG_CACHE_HOME")
        .output()
        .expect("run mitts test");
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out, text)
}

#[test]
fn test_cargo_bin_exe_isolation_and_no_target_debug_pollution() {
    let temp = tempdir().expect("tempdir");
    let home = tempdir().expect("homedir");
    let ws = temp.path();

    fs::write(
        ws.join("Cargo.toml"),
        r#"[package]
name = "isolation-demo"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "my-cli"
path = "src/main.rs"

[[test]]
name = "integration_test"
path = "tests/integration.rs"
"#,
    )
    .unwrap();

    fs::create_dir_all(ws.join("src")).unwrap();
    fs::write(
        ws.join("src/main.rs"),
        r#"fn main() {
    println!("isolated-cli-executed");
}
"#,
    )
    .unwrap();

    fs::create_dir_all(ws.join("tests")).unwrap();
    fs::write(
        ws.join("tests/integration.rs"),
        r#"use std::process::Command;

#[test]
fn test_bin_execution() {
    let bin_path = env!("CARGO_BIN_EXE_my-cli");
    assert!(
        bin_path.contains("target/mitts/debug/my-cli") || bin_path.contains("target/mitts/release/my-cli"),
        "CARGO_BIN_EXE_my-cli must point to target/mitts, got: {}",
        bin_path
    );
    let out = Command::new(bin_path).output().expect("execute binary");
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("isolated-cli-executed"));
}
"#,
    )
    .unwrap();

    let (out, log) = run_mitts_test(ws, home.path());
    assert!(
        out.status.success(),
        "mitts test failed with code {:?}. Output:\n{}",
        out.status.code(),
        log
    );

    let mitts_cli = ws.join("target/mitts/debug/my-cli");
    assert!(
        mitts_cli.exists(),
        "Expected binary at target/mitts/debug/my-cli"
    );

    let cargo_target_debug = ws.join("target/debug");
    assert!(
        !cargo_target_debug.exists() || !cargo_target_debug.join("my-cli").exists(),
        "Mitts must NOT create or link binaries in target/debug! Found: {:?}",
        cargo_target_debug.join("my-cli")
    );
}
