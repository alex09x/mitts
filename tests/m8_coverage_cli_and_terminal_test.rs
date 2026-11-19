/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::rust::inputs::hash_package_base_inputs;
use mitts::cli::coverage::{handle_coverage, CoverageArgs};
use mitts::cli::test::{handle_test, TestArgs};
use std::fs;
use std::sync::Mutex;

/// `HOME` and `CARGO_HOME` are process-wide: the home-config test rewrites them while the
/// coverage tests read the Cargo configuration, so the tests of this file take turns.
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[test]
fn test_m8_coverage_html_lcov_and_threshold_pass() {
    let _env = env_lock();
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    let manifest = root.join("Cargo.toml");
    fs::write(
        &manifest,
        r#"
[package]
name = "cov-reports-pkg"
version = "0.1.0"
edition = "2021"

[[test]]
name = "sample_test"
path = "tests/sample.rs"
"#,
    )
    .unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("src").join("lib.rs"),
        "pub fn compute_sum(a: i32, b: i32) -> i32 { a + b }\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("tests")).unwrap();
    fs::write(
        root.join("tests").join("sample.rs"),
        "#[test]\nfn test_sum() { assert_eq!(2 + 3, 5); }\n",
    )
    .unwrap();

    let out_dir = root.join("reports");

    // 1. Coverage with --html
    let html_args = CoverageArgs {
        manifest_path: Some(manifest.clone()),
        html: true,
        output_dir: Some(out_dir.clone()),
        fail_under_lines: Some(10.0), // Meets threshold
        cache_dir: Some(root.join(".mitts_cache")),
        target_dir: Some(root.join("target")),
        filter: Some("test_sum".into()),
        test_args: vec!["--nocapture".into()],
        ..Default::default()
    };
    let code_html = handle_coverage(&html_args);
    assert_eq!(code_html, 0);

    // 2. Coverage with --lcov
    let lcov_args = CoverageArgs {
        manifest_path: Some(manifest.clone()),
        lcov: true,
        output_dir: Some(out_dir),
        cache_dir: Some(root.join(".mitts_cache")),
        target_dir: Some(root.join("target")),
        ..Default::default()
    };
    let code_lcov = handle_coverage(&lcov_args);
    assert_eq!(code_lcov, 0);
}

#[test]
fn test_m8_test_and_coverage_no_tests_found() {
    let _env = env_lock();
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    // Package without tests
    let manifest = root.join("Cargo.toml");
    fs::write(
        &manifest,
        r#"
[package]
name = "pure-lib"
version = "0.1.0"
edition = "2021"
"#,
    )
    .unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src").join("lib.rs"), "pub fn f() {}\n").unwrap();

    let test_args = TestArgs {
        manifest_path: Some(manifest.clone()),
        cache_dir: Some(root.join(".mitts_cache")),
        target_dir: Some(root.join("target")),
        ..Default::default()
    };
    assert_eq!(handle_test(&test_args), 0);

    let cov_args = CoverageArgs {
        manifest_path: Some(manifest),
        cache_dir: Some(root.join(".mitts_cache")),
        target_dir: Some(root.join("target")),
        ..Default::default()
    };
    assert_eq!(handle_coverage(&cov_args), 0);
}

#[test]
fn test_m8_inputs_home_cargo_config_without_cargo_home() {
    let _env = env_lock();
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    // Create HOME/.cargo/config.toml
    let home_cargo = root.join("home").join(".cargo");
    fs::create_dir_all(&home_cargo).unwrap();
    fs::write(home_cargo.join("config.toml"), "[build]\njobs = 4\n").unwrap();

    let pkg_dir = root.join("my_pkg");
    fs::create_dir_all(&pkg_dir).unwrap();
    fs::write(
        pkg_dir.join("Cargo.toml"),
        "[package]\nname=\"my-pkg\"\nversion=\"0.1.0\"\n",
    )
    .unwrap();

    // Temporarily unset CARGO_HOME and set HOME
    let prev_cargo_home = std::env::var_os("CARGO_HOME");
    let prev_home = std::env::var_os("HOME");
    std::env::remove_var("CARGO_HOME");
    std::env::set_var("HOME", root.join("home").display().to_string());

    let res = hash_package_base_inputs(&pkg_dir, None, false);

    // Restore both: a `HOME` left pointing into the deleted temp dir broke the coverage tests
    // that read the Cargo configuration afterwards.
    if let Some(v) = prev_cargo_home {
        std::env::set_var("CARGO_HOME", v);
    }
    match prev_home {
        Some(v) => std::env::set_var("HOME", v),
        None => std::env::remove_var("HOME"),
    }

    assert!(res.is_ok());
    let inputs = res.unwrap();
    assert!(inputs.iter().any(|(k, _)| k.contains(".cargo")));
}
