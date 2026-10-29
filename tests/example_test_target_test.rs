/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! An `[[example]]` declared with `test = true` is a test target: `cargo test --tests`
//! compiles and runs it. clap declares `pacman` and `05_01_assert_derive` that way, and
//! Mitts built 22 test binaries where Cargo built 24.

use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

fn write_project(dir: &Path) {
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::create_dir_all(dir.join("examples")).unwrap();
    fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
         [[example]]\nname = \"tested\"\ntest = true\n\n[[example]]\nname = \"plain\"\n",
    )
    .unwrap();
    fs::write(dir.join("src/lib.rs"), "pub fn v() -> u32 { 42 }\n").unwrap();
    fs::write(
        dir.join("examples/tested.rs"),
        "fn main() { println!(\"{}\", app::v()); }\n#[test]\nfn example_answer() { assert_eq!(app::v(), 42); }\n",
    )
    .unwrap();
    fs::write(
        dir.join("examples/plain.rs"),
        "fn main() {}\n#[test]\nfn never_selected() { panic!(\"plain examples are not tests\"); }\n",
    )
    .unwrap();
}

#[test]
fn tested_examples_run_with_tests_and_plain_ones_do_not() {
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
    assert!(out.status.success(), "the tested example passes:\n{text}");
    assert!(text.contains("example_answer"), "the example's test ran:\n{text}");
    assert!(!text.contains("never_selected"), "a plain example is not a test:\n{text}");
}
