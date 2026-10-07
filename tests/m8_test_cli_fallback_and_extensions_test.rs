/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::cli::test::{handle_test, TestArgs};
use std::fs;
use tempfile::TempDir;

#[test]
fn test_m8_test_cli_profile_dir_scanning_and_extension_skips() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    let target = root.join("target");
    let debug_dir = target.join("debug");
    let manifest = root.join("Cargo.toml");
    let src = root.join("src");

    fs::create_dir_all(&debug_dir).unwrap();
    fs::create_dir_all(&src).unwrap();

    fs::write(
        &manifest,
        "[package]\nname = \"test-scan-pkg\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(src.join("lib.rs"), "pub fn hello() {}\n").unwrap();

    // Populate debug dir with ignorable extensions and valid test executable
    fs::write(debug_dir.join("libtest.d"), "dep info").unwrap();
    fs::write(debug_dir.join("libtest.rmeta"), "meta").unwrap();
    fs::write(debug_dir.join("libtest.rlib"), "rlib").unwrap();
    fs::write(debug_dir.join("test.pdb"), "pdb").unwrap();

    let valid_test = debug_dir.join("custom_unit_test");
    fs::write(&valid_test, "#!/bin/sh\nexit 0\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&valid_test, fs::Permissions::from_mode(0o755));
    }

    // 1. Run with --no-run
    let args_no_run = TestArgs {
        manifest_path: Some(manifest.clone()),
        target_dir: Some(target.clone()),
        no_run: true,
        ..Default::default()
    };
    let code_no_run = handle_test(&args_no_run);
    assert_eq!(code_no_run, 0);

    // 2. Run with test execution
    let args_run = TestArgs {
        manifest_path: Some(manifest.clone()),
        target_dir: Some(target.clone()),
        no_run: false,
        ..Default::default()
    };
    let code_run = handle_test(&args_run);
    assert_eq!(code_run, 0);

    // 3. Test failure exit code
    let failing_test = debug_dir.join("failing_test");
    fs::write(&failing_test, "#!/bin/sh\nexit 42\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&failing_test, fs::Permissions::from_mode(0o755));
    }

    let code_fail = handle_test(&args_run);
    assert!(code_fail != 0);
}

#[test]
fn test_m8_test_cli_empty_tests_found() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    let target = root.join("target");
    let manifest = root.join("Cargo.toml");
    let src = root.join("src");

    fs::create_dir_all(&src).unwrap();
    fs::write(
        &manifest,
        "[package]\nname = \"no-tests\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(src.join("lib.rs"), "pub fn compute() -> i32 { 42 }\n").unwrap();

    let args = TestArgs {
        manifest_path: Some(manifest),
        target_dir: Some(target),
        no_run: false,
        ..Default::default()
    };

    let code = handle_test(&args);
    assert_eq!(code, 0);
}
