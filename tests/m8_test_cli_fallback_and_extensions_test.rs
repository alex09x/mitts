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

    // 3. Stale or rogue executable in debug_dir not in build plan must NOT be executed
    let rogue_test = debug_dir.join("rogue_stale_test");
    fs::write(&rogue_test, "#!/bin/sh\nexit 42\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&rogue_test, fs::Permissions::from_mode(0o755));
    }

    let code_after_rogue = handle_test(&args_run);
    assert_eq!(
        code_after_rogue, 0,
        "Stale or rogue binaries in target dir not belonging to plan must not be executed"
    );
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

#[test]
fn test_m8_consecutive_test_runs_with_failing_fixture_must_fail_both_times() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    let target = root.join("target");
    let manifest = root.join("Cargo.toml");
    let src = root.join("src");
    let tests_dir = root.join("tests");

    fs::create_dir_all(&src).unwrap();
    fs::create_dir_all(&tests_dir).unwrap();

    fs::write(
        &manifest,
        "[package]\nname = \"failing-fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[[test]]\nname = \"failing_test\"\npath = \"tests/fail.rs\"\n",
    )
    .unwrap();
    fs::write(src.join("lib.rs"), "pub fn ok() {}\n").unwrap();
    fs::write(
        tests_dir.join("fail.rs"),
        "#[test]\nfn test_failure() {\n    panic!(\"explicit failure for regression verification\");\n}\n",
    )
    .unwrap();

    let args = TestArgs {
        manifest_path: Some(manifest),
        target_dir: Some(target),
        no_run: false,
        ..Default::default()
    };

    // First run: compiles real test and executes; must fail due to panic in test
    let code_first = handle_test(&args);
    assert_ne!(code_first, 0, "First run on failing fixture must fail");

    // Second consecutive run: must STILL fail (fast-lock must NOT exit 0 or skip execution)
    let code_second = handle_test(&args);
    assert_ne!(
        code_second, 0,
        "Second consecutive run on failing fixture must also fail"
    );
}

#[test]
fn test_m8_test_transition_from_pass_to_fail_must_fail_on_second_run() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    let target = root.join("target");
    let manifest = root.join("Cargo.toml");
    let src = root.join("src");
    let tests_dir = root.join("tests");

    fs::create_dir_all(&src).unwrap();
    fs::create_dir_all(&tests_dir).unwrap();

    fs::write(
        &manifest,
        "[package]\nname = \"dynamic-fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[[test]]\nname = \"dyn_test\"\npath = \"tests/dynamic.rs\"\n",
    )
    .unwrap();
    fs::write(src.join("lib.rs"), "pub fn val() -> bool { true }\n").unwrap();
    fs::write(
        tests_dir.join("dynamic.rs"),
        "#[test]\nfn test_pass() {\n    assert!(true);\n}\n",
    )
    .unwrap();

    let args = TestArgs {
        manifest_path: Some(manifest),
        target_dir: Some(target),
        no_run: false,
        ..Default::default()
    };

    // First run: passing test; must exit 0
    let code_first = handle_test(&args);
    assert_eq!(code_first, 0, "First run on passing fixture must succeed");

    // Modify test file to panic
    fs::write(
        tests_dir.join("dynamic.rs"),
        "#[test]\nfn test_pass() {\n    panic!(\"modified into failing test\");\n}\n",
    )
    .unwrap();

    // Second run: must recompile and fail; cannot exit 0
    let code_second = handle_test(&args);
    assert_ne!(
        code_second, 0,
        "Second run after modifying test to panic must fail"
    );
}
