/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Cargo finds `src/bin/*.rs` also when `[[bin]]` tables exist, unless `autobins = false`. Mitts
//! discovered them only in a package without tables, so `mitts test --bin auto` failed with
//! "no bin target named `auto`" and `mitts build` did not build the binary (issue 41).

use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use tempfile::TempDir;

fn write(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

fn project(autobins: &str) -> TempDir {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    write(
        &root.join("Cargo.toml"),
        &format!(
            "[package]\nname = \"autobin-repro\"\nversion = \"0.1.0\"\nedition = \"2021\"\n{autobins}\n\
             [[bin]]\nname = \"explicit\"\npath = \"src/bin/explicit.rs\"\n"
        ),
    );
    write(&root.join("src/bin/explicit.rs"), "fn main() {}\n");
    write(
        &root.join("src/bin/auto.rs"),
        "fn main() {}\n#[test]\nfn smoke() { assert!(true); }\n",
    );
    dir
}

fn mitts(dir: &TempDir, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mitts"))
        .current_dir(dir.path())
        .args(args)
        .arg("--cache-dir")
        .arg(dir.path().join("cache"))
        .env("CARGO_HOME", dir.path().join("cargo-home"))
        .output()
        .unwrap()
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn test_bin_selects_an_inferred_binary_next_to_a_table() {
    let dir = project("");
    let output = mitts(
        &dir,
        &["test", "--offline", "-j", "1", "--bin", "auto", "smoke"],
    );
    assert!(output.status.success(), "{}", text(&output));
    assert!(
        text(&output).contains("test smoke ... ok"),
        "{}",
        text(&output)
    );
}

#[test]
fn build_produces_the_inferred_binary_and_the_table_binary() {
    let dir = project("");
    let output = mitts(&dir, &["build", "--offline"]);
    assert!(output.status.success(), "{}", text(&output));
    for name in ["explicit", "auto"] {
        assert!(
            dir.path().join("target/debug").join(name).is_file(),
            "{name} is not built:\n{}",
            text(&output)
        );
    }
}

#[test]
fn autobins_false_keeps_only_the_table() {
    let dir = project("autobins = false\n");
    let output = mitts(&dir, &["build", "--offline"]);
    assert!(output.status.success(), "{}", text(&output));
    assert!(dir.path().join("target/debug/explicit").is_file());
    assert!(!dir.path().join("target/debug/auto").exists());
}
