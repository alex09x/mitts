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
use std::process::Command;
use tempfile::tempdir;

static SERIAL_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

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

#[test]
fn test_custom_harness_and_target_filters() {
    let _lock = SERIAL_TEST_LOCK.lock().unwrap();
    let dir = tempdir().expect("tempdir");
    let ws = dir.path();

    // Create a crate with:
    // 1. [lib] with doctest and unit test
    // 2. [[bin]] with unit test
    // 3. [[test]] standard integration test (harness = true)
    // 4. [[test]] custom harness integration test (harness = false)
    fs::create_dir_all(ws.join("src/bin")).unwrap();
    fs::create_dir_all(ws.join("tests")).unwrap();

    fs::write(
        ws.join("Cargo.toml"),
        r#"[package]
name = "harness-demo"
version = "0.1.0"
edition = "2021"

[lib]
path = "src/lib.rs"

[[bin]]
name = "my-cli"
path = "src/bin/main.rs"

[[test]]
name = "std_test"
path = "tests/std_test.rs"

[[test]]
name = "custom_test"
path = "tests/custom_test.rs"
harness = false
"#,
    )
    .unwrap();

    fs::write(
        ws.join("src/lib.rs"),
        r#"//! # Library with doctest
//! ```
//! assert_eq!(harness_demo::add(1, 2), 3);
//! ```
pub fn add(a: i32, b: i32) -> i32 {
    a + b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lib_add() {
        assert_eq!(add(2, 2), 4);
    }
}
"#,
    )
    .unwrap();

    fs::write(
        ws.join("src/bin/main.rs"),
        r#"fn main() {
    println!("Hello from cli");
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_bin() {
        assert_eq!(2 * 2, 4);
    }
}
"#,
    )
    .unwrap();

    fs::write(
        ws.join("tests/std_test.rs"),
        r#"#[test]
fn test_integration_std() {
    assert_eq!(harness_demo::add(10, 20), 30);
}
"#,
    )
    .unwrap();

    fs::write(
        ws.join("tests/custom_test.rs"),
        r#"fn main() {
    println!("Running custom harness test without libtest!");
    assert_eq!(harness_demo::add(5, 5), 10);
}
"#,
    )
    .unwrap();

    let temp_home = dir.path().join("home");
    fs::create_dir_all(&temp_home).unwrap();
    let bin = mitts_bin();

    // 1. Run all tests (should execute lib test, bin test, std_test, custom_test, and doctests)
    let out = Command::new(&bin)
        .arg("test")
        .current_dir(ws)
        .env("HOME", &temp_home)
        .env_remove("MITTS_CACHE_DIR")
        .env_remove("XDG_CACHE_HOME")
        .output()
        .expect("mitts test");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let full_out = format!("{}\n{}", stdout, stderr);

    assert!(
        out.status.success(),
        "All tests should pass. Output:\n{}",
        full_out
    );
    assert!(
        full_out.contains("Running custom harness test without libtest!"),
        "Custom harness main() must execute! Output:\n{}",
        full_out
    );
    assert!(
        full_out.contains("test_lib_add"),
        "Lib unit test must run! Output:\n{}",
        full_out
    );
    assert!(
        full_out.contains("test_bin"),
        "Bin unit test must run! Output:\n{}",
        full_out
    );
    assert!(
        full_out.contains("test_integration_std"),
        "Std integration test must run! Output:\n{}",
        full_out
    );
    assert!(
        full_out.contains("Doc-tests harness_demo"),
        "Doctests must run! Output:\n{}",
        full_out
    );

    // 2. Test target filter: --test std_test (should only run std_test, no doctests, no lib tests)
    let out = Command::new(&bin)
        .arg("test")
        .arg("--test")
        .arg("std_test")
        .current_dir(ws)
        .env("HOME", &temp_home)
        .env_remove("MITTS_CACHE_DIR")
        .env_remove("XDG_CACHE_HOME")
        .output()
        .expect("mitts test --test std_test");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let full_out = format!("{}\n{}", stdout, stderr);

    assert!(
        out.status.success(),
        "mitts test --test std_test should pass! Output:\n{}",
        full_out
    );
    assert!(full_out.contains("test_integration_std"));
    assert!(!full_out.contains("test_lib_add"));
    assert!(!full_out.contains("Running custom harness test without libtest!"));
    assert!(!full_out.contains("Doc-tests"));

    // 3. Test target filter: --lib (should only run lib unit test)
    let out = Command::new(&bin)
        .arg("test")
        .arg("--lib")
        .current_dir(ws)
        .env("HOME", &temp_home)
        .env_remove("MITTS_CACHE_DIR")
        .env_remove("XDG_CACHE_HOME")
        .output()
        .expect("mitts test --lib");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let full_out = format!("{}\n{}", stdout, stderr);

    assert!(
        out.status.success(),
        "mitts test --lib should pass! Output:\n{}",
        full_out
    );
    assert!(full_out.contains("test_lib_add"));
    assert!(!full_out.contains("test_integration_std"));
    assert!(!full_out.contains("Running custom harness test without libtest!"));
    assert!(!full_out.contains("Doc-tests"));

    // 4. Test target filter: --doc (should only run doctests)
    let out = Command::new(&bin)
        .arg("test")
        .arg("--doc")
        .current_dir(ws)
        .env("HOME", &temp_home)
        .env_remove("MITTS_CACHE_DIR")
        .env_remove("XDG_CACHE_HOME")
        .output()
        .expect("mitts test --doc");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let full_out = format!("{}\n{}", stdout, stderr);

    assert!(
        out.status.success(),
        "mitts test --doc should pass! Output:\n{}",
        full_out
    );
    assert!(full_out.contains("Doc-tests harness_demo"));
    assert!(!full_out.contains("test_lib_add"));
    assert!(!full_out.contains("test_integration_std"));

    // 5. Test runner args: --nocapture and test filter
    let out = Command::new(&bin)
        .arg("test")
        .arg("--lib")
        .arg("test_lib_add")
        .arg("--nocapture")
        .current_dir(ws)
        .env("HOME", &temp_home)
        .env_remove("MITTS_CACHE_DIR")
        .env_remove("XDG_CACHE_HOME")
        .output()
        .expect("mitts test --lib test_lib_add --nocapture");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let full_out = format!("{}\n{}", stdout, stderr);

    assert!(
        out.status.success(),
        "mitts test with filter and --nocapture should pass! Output:\n{}",
        full_out
    );
    assert!(full_out.contains("test_lib_add"));
}
