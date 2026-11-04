/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::core::lock::FileLock;
use std::path::PathBuf;
use std::process::Command;

#[test]
fn test_m8_main_binary_dispatch_and_subcommands() {
    let bin_path = env!("CARGO_BIN_EXE_mitts");

    // 1. Help outputs for all subcommands (exercises main.rs Cli matching)
    let subcommands = [
        "build", "check", "clippy", "fmt", "test", "coverage", "llvm-cov", "run", "explain",
        "fetch", "gc", "clean", "metadata", "rustc",
    ];
    for sub in subcommands {
        let output = Command::new(bin_path)
            .arg(sub)
            .arg("--help")
            .output()
            .unwrap();
        assert!(output.status.success(), "{sub} --help failed");
        assert!(String::from_utf8_lossy(&output.stdout).contains("Usage: mitts"));
    }

    // 2. Dispatch successful command through main
    let fixture_manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("hello-rust")
        .join("Cargo.toml");
    let output_clean = Command::new(bin_path)
        .arg("clean")
        .arg("--manifest-path")
        .arg(&fixture_manifest)
        .output()
        .unwrap();
    assert!(output_clean.status.success());

    // 3. Dispatch failing command through main (tests ExitCode::from(code as u8))
    let output_err = Command::new(bin_path)
        .arg("clean")
        .arg("--manifest-path")
        .arg("/nonexistent/path/Cargo.toml")
        .output()
        .unwrap();
    assert!(!output_err.status.success());
}

#[test]
fn test_m8_file_lock_shared_exclusive_and_path() {
    let temp = tempfile::tempdir().unwrap();
    let lock_path = temp.path().join("nested").join("test.lock");

    // 1. acquire_exclusive creates parent dir and returns lock
    let lock1 = FileLock::acquire_exclusive(&lock_path).unwrap();
    assert_eq!(lock1.path(), &lock_path);

    // 2. try_acquire_exclusive while locked returns None
    let try_lock = FileLock::try_acquire_exclusive(&lock_path).unwrap();
    assert!(try_lock.is_none());

    drop(lock1);

    // 3. acquire_shared
    let shared1 = FileLock::acquire_shared(&lock_path).unwrap();
    let shared2 = FileLock::acquire_shared(&lock_path).unwrap();
    assert_eq!(shared1.path(), &lock_path);
    assert_eq!(shared2.path(), &lock_path);
}
