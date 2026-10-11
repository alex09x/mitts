/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Integration tests covering target filtering combinations, validation errors (exit code 101),
//! failing doctests, and [lib] harness = false custom harnesses.

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

fn create_test_project(ws: &Path) {
    fs::create_dir_all(ws.join("src/bin")).unwrap();
    fs::create_dir_all(ws.join("tests")).unwrap();

    fs::write(
        ws.join("Cargo.toml"),
        r#"[package]
name = "filter-demo"
version = "0.1.0"
edition = "2021"

[lib]
path = "src/lib.rs"

[[bin]]
name = "cli-tool"
path = "src/bin/main.rs"

[[test]]
name = "integration_one"
path = "tests/integration_one.rs"
"#,
    )
    .unwrap();

    fs::write(
        ws.join("src/lib.rs"),
        r#"pub fn add(a: i32, b: i32) -> i32 { a + b }

#[cfg(test)]
mod tests {
    #[test]
    fn test_in_lib() { assert_eq!(2 + 2, 4); }
}
"#,
    )
    .unwrap();

    fs::write(
        ws.join("src/bin/main.rs"),
        r#"fn main() {}

#[cfg(test)]
mod tests {
    #[test]
    fn test_in_bin() { assert_eq!(3 * 3, 9); }
}
"#,
    )
    .unwrap();

    fs::write(
        ws.join("tests/integration_one.rs"),
        r#"#[test]
fn test_integration() { assert_eq!(filter_demo::add(1, 1), 2); }
"#,
    )
    .unwrap();
}

#[test]
fn test_unknown_targets_and_invalid_options_fail_101() {
    let dir = tempdir().expect("tempdir");
    let ws = dir.path().join("ws");
    let home = dir.path().join("home");
    fs::create_dir_all(&home).unwrap();
    create_test_project(&ws);

    // 1. Unknown integration test -> exit 101 with error message
    let (out, text) = run_mitts(&ws, &home, &["--test", "nosuch"]);
    assert_eq!(out.status.code(), Some(101), "output:\n{}", text);
    assert!(
        text.contains("no test target named `nosuch`"),
        "output:\n{}",
        text
    );

    // 2. Unknown bin target -> exit 101 with error message
    let (out, text) = run_mitts(&ws, &home, &["--bin", "nosuch"]);
    assert_eq!(out.status.code(), Some(101), "output:\n{}", text);
    assert!(
        text.contains("no bin target named `nosuch`"),
        "output:\n{}",
        text
    );

    // 3. Mixing --doc with --test -> exit 101
    let (out, text) = run_mitts(&ws, &home, &["--doc", "--test", "integration_one"]);
    assert_eq!(out.status.code(), Some(101), "output:\n{}", text);
    assert!(
        text.contains("can't mix --doc with other target selecting options"),
        "output:\n{}",
        text
    );
}

#[test]
fn test_filter_combinations_and_tests_flag() {
    let dir = tempdir().expect("tempdir");
    let ws = dir.path().join("ws");
    let home = dir.path().join("home");
    fs::create_dir_all(&home).unwrap();
    create_test_project(&ws);

    // 1. --tests: runs all test targets (lib unit test + bin unit test + integration test)
    let (out, text) = run_mitts(&ws, &home, &["--tests"]);
    assert!(out.status.success(), "output:\n{}", text);
    assert!(text.contains("test_in_lib"), "output:\n{}", text);
    assert!(text.contains("test_in_bin"), "output:\n{}", text);
    assert!(text.contains("test_integration"), "output:\n{}", text);

    // 2. --bins: runs only binary unit tests
    let (out, text) = run_mitts(&ws, &home, &["--bins"]);
    assert!(out.status.success(), "output:\n{}", text);
    assert!(text.contains("test_in_bin"), "output:\n{}", text);
    assert!(!text.contains("test_in_lib"), "output:\n{}", text);
    assert!(!text.contains("test_integration"), "output:\n{}", text);

    // 3. --bin cli-tool: runs specific binary unit test
    let (out, text) = run_mitts(&ws, &home, &["--bin", "cli-tool"]);
    assert!(out.status.success(), "output:\n{}", text);
    assert!(text.contains("test_in_bin"), "output:\n{}", text);
    assert!(!text.contains("test_in_lib"), "output:\n{}", text);

    // 4. Union combinations: --lib --test integration_one runs both
    let (out, text) = run_mitts(&ws, &home, &["--lib", "--test", "integration_one"]);
    assert!(out.status.success(), "output:\n{}", text);
    assert!(text.contains("test_in_lib"), "output:\n{}", text);
    assert!(text.contains("test_integration"), "output:\n{}", text);
    assert!(!text.contains("test_in_bin"), "output:\n{}", text);
}

#[test]
fn test_failing_doctest_exits_nonzero() {
    let dir = tempdir().expect("tempdir");
    let ws = dir.path().join("ws");
    let home = dir.path().join("home");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(ws.join("src")).unwrap();

    fs::write(
        ws.join("Cargo.toml"),
        r#"[package]
name = "bad-doc"
version = "0.1.0"
edition = "2021"
"#,
    )
    .unwrap();

    fs::write(
        ws.join("src/lib.rs"),
        r#"/// ```
/// assert_eq!(1, 2);
/// ```
pub fn broken() {}
"#,
    )
    .unwrap();

    let (out, text) = run_mitts(&ws, &home, &["--doc"]);
    assert!(
        !out.status.success(),
        "failing doctest must return non-zero: \n{}",
        text
    );
    assert!(text.contains("Doc-tests bad_doc"), "output:\n{}", text);
}

#[test]
fn test_lib_custom_harness_false() {
    let dir = tempdir().expect("tempdir");
    let ws = dir.path().join("ws");
    let home = dir.path().join("home");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(ws.join("src")).unwrap();

    fs::write(
        ws.join("Cargo.toml"),
        r#"[package]
name = "custom-lib"
version = "0.1.0"
edition = "2021"

[lib]
harness = false
"#,
    )
    .unwrap();

    fs::write(
        ws.join("src/lib.rs"),
        r#"fn main() {
    println!("custom-lib-harness-executed");
}
"#,
    )
    .unwrap();

    let (out, text) = run_mitts(&ws, &home, &["--lib"]);
    assert!(
        out.status.success(),
        "custom harness lib must succeed: \n{}",
        text
    );
    assert!(
        text.contains("custom-lib-harness-executed"),
        "output:\n{}",
        text
    );
}
