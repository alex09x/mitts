/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Doctests must link the exact artifacts of the build: the library, its renamed
//! dev-dependency, and its enabled features — never a same-named file found by scanning.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use tempfile::tempdir;

fn mitts_bin() -> std::path::PathBuf {
    std::env::var("CARGO_BIN_EXE_mitts")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            let mut p = std::env::current_exe().expect("current exe");
            p.pop();
            if p.ends_with("deps") {
                p.pop();
            }
            p.join("mitts")
        })
}

fn run_mitts(ws: &Path, home: &Path, args: &[&str]) -> (Output, String) {
    let out = Command::new(mitts_bin())
        .arg("test")
        .args(args)
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

fn write_fixture(ws: &Path) {
    fs::create_dir_all(ws.join("src")).unwrap();
    fs::create_dir_all(ws.join("doc-helper/src")).unwrap();
    fs::write(
        ws.join("Cargo.toml"),
        r#"[workspace]
members = ["doc-helper"]

[package]
name = "doc-demo"
version = "0.1.0"
edition = "2021"

[features]
default = ["extra"]
extra = []

[dev-dependencies]
aid = { package = "doc-helper", path = "doc-helper" }
"#,
    )
    .unwrap();
    // Cargo-generated lockfile: Mitts' native lock generation does not yet record
    // path dev-dependencies, which is tracked separately from doctest linking.
    fs::write(
        ws.join("Cargo.lock"),
        r#"version = 4

[[package]]
name = "doc-demo"
version = "0.1.0"
dependencies = [
 "doc-helper",
]

[[package]]
name = "doc-helper"
version = "0.1.0"
"#,
    )
    .unwrap();
    fs::write(
        ws.join("src/lib.rs"),
        r#"/// ```
/// assert_eq!(aid::answer(), 42);
/// ```
pub fn uses_dev_dep() {}

/// ```
/// assert_eq!(doc_demo::extra(), 7);
/// ```
#[cfg(feature = "extra")]
pub fn extra() -> u32 {
    7
}

/// ```
/// println!("doctest-says-hello");
/// ```
pub fn hello() {}
"#,
    )
    .unwrap();
    fs::write(
        ws.join("doc-helper/Cargo.toml"),
        r#"[package]
name = "doc-helper"
version = "0.1.0"
edition = "2021"
"#,
    )
    .unwrap();
    fs::write(
        ws.join("doc-helper/src/lib.rs"),
        "pub fn answer() -> u32 {\n    42\n}\n",
    )
    .unwrap();
}

#[test]
fn test_doctests_link_exact_dev_deps_features_and_ignore_decoys() {
    let dir = tempdir().expect("tempdir");
    let ws = dir.path().join("ws");
    let home = dir.path().join("home");
    fs::create_dir_all(&home).unwrap();
    write_fixture(&ws);

    // 1. Renamed dev-dependency and feature-gated item are both visible to doctests.
    let (out, text) = run_mitts(&ws, &home, &["--doc"]);
    assert!(out.status.success(), "doctests must pass:\n{}", text);
    assert!(
        text.contains("uses_dev_dep (line"),
        "dev-dep doctest ran:\n{}",
        text
    );
    assert!(
        text.contains("extra (line"),
        "feature-gated doctest ran:\n{}",
        text
    );
    assert!(
        text.contains("3 passed"),
        "all three doctests pass:\n{}",
        text
    );

    // 2. A newer same-named decoy next to real artifacts must not be linked.
    let deps = ws.join("target/mitts/debug/deps");
    fs::create_dir_all(&deps).unwrap();
    fs::write(deps.join("libdoc_helper-ffffffffffffffff.rlib"), b"decoy").unwrap();
    fs::write(deps.join("libdoc_demo-ffffffffffffffff.rlib"), b"decoy").unwrap();
    let (out, text) = run_mitts(&ws, &home, &["--doc"]);
    assert!(out.status.success(), "decoys must be ignored:\n{}", text);
    assert!(text.contains("3 passed"), "same doctests pass:\n{}", text);

    // 3. Filter and --nocapture are forwarded to the doctest runner. rustdoc keeps
    //    doctest stdout captured either way (same as `cargo test --doc -- --nocapture`).
    let (out, text) = run_mitts(&ws, &home, &["--doc", "hello", "--nocapture"]);
    assert!(
        out.status.success(),
        "filtered doctest must pass:\n{}",
        text
    );
    assert!(text.contains("1 passed"), "one doctest selected:\n{}", text);
    assert!(
        text.contains("2 filtered out"),
        "others filtered:\n{}",
        text
    );

    // 4. Trailing runner args reach libtest: `--list` lists doctests instead of running them.
    let (out, text) = run_mitts(&ws, &home, &["--doc", "--", "--list"]);
    assert!(out.status.success(), "--list must succeed:\n{}", text);
    assert!(text.contains("hello (line"), "doctests listed:\n{}", text);
    assert!(
        !text.contains("passed"),
        "nothing executed with --list:\n{}",
        text
    );
}
