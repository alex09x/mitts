/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::c::{CAdapter, CBuildOptions};
use mitts::cli::handlers::{handle_clean, handle_explain, handle_fetch, handle_gc};
use mitts::cli::{CleanArgs, ExplainArgs, FetchArgs, GcArgs};
use std::fs;
use tempfile::TempDir;

#[test]
fn test_m8_handlers_clean_release_and_dev() {
    let tmp = TempDir::new().unwrap();
    let pkg_dir = tmp.path().join("pkg");
    fs::create_dir_all(&pkg_dir).unwrap();

    let cargo_toml = pkg_dir.join("Cargo.toml");
    fs::write(
        &cargo_toml,
        "[package]\nname = \"clean-pkg\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();

    let mitts_dir = pkg_dir.join(".mitts");
    fs::create_dir_all(&mitts_dir).unwrap();
    fs::write(mitts_dir.join("build-dev.lock"), "lock").unwrap();
    fs::write(mitts_dir.join("build-release.lock"), "lock").unwrap();

    // 1. Clean release
    let clean_rel = CleanArgs {
        manifest_path: Some(cargo_toml.clone()),
        release: true,
        ..Default::default()
    };
    assert_eq!(handle_clean(&clean_rel), 0);
    assert!(!mitts_dir.join("build-release.lock").exists());
    assert!(mitts_dir.join("build-dev.lock").exists());

    // 2. Clean dev (all)
    let clean_dev = CleanArgs {
        manifest_path: Some(cargo_toml.clone()),
        release: false,
        ..Default::default()
    };
    assert_eq!(handle_clean(&clean_dev), 0);
    assert!(!mitts_dir.join("build-dev.lock").exists());
}

#[test]
fn test_m8_handlers_explain_and_gc_defaults() {
    let tmp = TempDir::new().unwrap();

    // 1. handle_explain with missing manifest
    let explain_missing = ExplainArgs {
        manifest_path: Some(tmp.path().join("NonExistent.toml")),
        ..Default::default()
    };
    assert_eq!(handle_explain(&explain_missing), 1);

    // 2. handle_gc with explicit cache_dir
    let gc_explicit = GcArgs {
        cache_dir: Some(tmp.path().join("cache")),
        max_bytes: Some(1024),
        max_age_secs: Some(3600),
    };
    assert_eq!(handle_gc(&gc_explicit), 0);

    // 3. handle_fetch with missing manifest
    let fetch_missing = FetchArgs {
        manifest_path: Some(tmp.path().join("NonExistent.toml")),
        ..Default::default()
    };
    assert_eq!(handle_fetch(&fetch_missing), 1);
}

#[test]
fn test_m8_c_adapter_toolchain_and_build_errors() {
    let tmp = TempDir::new().unwrap();

    // 1. Toolchain detection
    let tc = CAdapter::detect_toolchain();
    assert!(tc.is_ok());

    // 2. C build with missing manifest
    let opt_missing = CBuildOptions {
        manifest_path: Some(tmp.path().join("NonExistent.toml")),
        ..Default::default()
    };
    assert!(CAdapter::build(&opt_missing).is_err());

    // 3. C build with valid C manifest and compilation error
    let c_dir = tmp.path().join("c-pkg");
    fs::create_dir_all(c_dir.join("src")).unwrap();

    let c_toml = c_dir.join("Mitts.toml");
    fs::write(
        &c_toml,
        "[package]\nname = \"c-fail\"\nversion = \"0.1.0\"\n\n[bin]\nsrc = \"src/main.c\"\n",
    )
    .unwrap();

    fs::write(
        c_dir.join("src").join("main.c"),
        "int main() { syntax error; }",
    )
    .unwrap();

    let opt_fail = CBuildOptions {
        manifest_path: Some(c_toml.clone()),
        target_dir: Some(tmp.path().join("target")),
        cache_dir: Some(tmp.path().join("cache")),
        ..Default::default()
    };
    let res = CAdapter::build(&opt_fail);
    assert!(res.is_err());

    // 4. Successful C build
    fs::write(
        c_dir.join("src").join("main.c"),
        "int main(void) { return 0; }\n",
    )
    .unwrap();
    let opt_ok = CBuildOptions {
        manifest_path: Some(c_toml),
        target_dir: Some(tmp.path().join("target")),
        cache_dir: Some(tmp.path().join("cache")),
        release: true,
        ..Default::default()
    };
    let res_ok = CAdapter::build(&opt_ok);
    assert!(res_ok.is_ok());
    let (plan_ok, action_ok) = res_ok.unwrap();
    assert!(action_ok.success);
    assert!(!plan_ok.actions.is_empty());
}
