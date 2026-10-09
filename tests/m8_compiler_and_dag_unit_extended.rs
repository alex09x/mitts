/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::rust::compiler::{resolve_clippy_driver, RustcInvocation};
use mitts::adapter::rust::types::{BuildMode, PlannedCompilationUnit};
use mitts::cli::build::handle_build;
use mitts::cli::BuildArgs;
use std::fs;
use tempfile::TempDir;

#[test]
fn test_m8_resolve_clippy_driver_env() {
    let tmp = TempDir::new().unwrap();
    let fake_clippy = tmp.path().join("fake_clippy");
    let old_clippy = std::env::var("CLIPPY_DRIVER").ok();

    std::env::set_var("CLIPPY_DRIVER", &fake_clippy);
    let resolved = resolve_clippy_driver();
    assert_eq!(resolved, fake_clippy);

    if let Some(c) = old_clippy {
        std::env::set_var("CLIPPY_DRIVER", c);
    } else {
        std::env::remove_var("CLIPPY_DRIVER");
    }
}

#[test]
fn test_m8_compiler_invocation_modes_and_non_semver() {
    let tmp = TempDir::new().unwrap();
    let staging_dir = tmp.path().join("stage");
    fs::create_dir_all(&staging_dir).unwrap();

    let src_file = tmp.path().join("lib.rs");
    fs::write(&src_file, "pub fn add(a: i32, b: i32) -> i32 { a + b }\n").unwrap();

    let manifest_path = tmp.path().join("Cargo.toml");
    fs::write(
        &manifest_path,
        "[package]\nname = \"dummy-lib\"\nversion = \"1.2.3-beta\"\n",
    )
    .unwrap();

    // 1. Non-standard semver version string fallback branch
    let unit_non_semver = PlannedCompilationUnit {
        package_id: "dummy-lib 1.2.3-beta".to_string(),
        package_name: "dummy-lib".to_string(),
        version: "non_standard_version".to_string(),
        target_name: "dummy_lib".to_string(),
        kind: "lib".to_string(),
        src_path: src_file.clone(),
        manifest_path: manifest_path.clone(),
        dependencies: Vec::new(),
        features: Vec::new(),
        env_vars: Vec::new(),
        cache_eligibility: "eligible".to_string(),
        is_workspace: true,
        is_primary: false,
        edition: Some("2021".to_string()),
    };

    let inv = RustcInvocation {
        unit: &unit_non_semver,
        action_key: "action_key_1234567890abcdef",
        out_dir: &staging_dir,
        dest_binary: None,
        extern_crates: &[],
        release: false,
        features: &[],
        directives: None,
        mode: BuildMode::Build,
        extra_compiler_args: &[],
        jobserver_auth: None,
        deps_dir: None,
    };

    let out = inv.execute().unwrap();
    assert!(out.success);

    // 2. Check mode on workspace member
    let inv_check = RustcInvocation {
        unit: &unit_non_semver,
        action_key: "action_key_check",
        out_dir: &staging_dir,
        dest_binary: None,
        extern_crates: &[],
        release: false,
        features: &[],
        directives: None,
        mode: BuildMode::Check,
        extra_compiler_args: &[],
        jobserver_auth: None,
        deps_dir: None,
    };
    let out_check = inv_check.execute().unwrap();
    assert!(out_check.success);

    // 3. Proc-macro kind
    let pm_src = tmp.path().join("pm.rs");
    fs::write(
        &pm_src,
        "extern crate proc_macro; use proc_macro::TokenStream; #[proc_macro] pub fn my_macro(input: TokenStream) -> TokenStream { input }\n",
    )
    .unwrap();

    let unit_pm = PlannedCompilationUnit {
        package_id: "dummy-pm 0.1.0".to_string(),
        package_name: "dummy-pm".to_string(),
        version: "0.1.0".to_string(),
        target_name: "dummy_pm".to_string(),
        kind: "proc-macro".to_string(),
        src_path: pm_src,
        manifest_path: manifest_path.clone(),
        dependencies: Vec::new(),
        features: Vec::new(),
        env_vars: Vec::new(),
        cache_eligibility: "eligible".to_string(),
        is_workspace: true,
        is_primary: false,
        edition: Some("2021".to_string()),
    };

    let inv_pm = RustcInvocation {
        unit: &unit_pm,
        action_key: "action_key_pm",
        out_dir: &staging_dir,
        dest_binary: None,
        extern_crates: &[],
        release: false,
        features: &[],
        directives: None,
        mode: BuildMode::Build,
        extra_compiler_args: &[],
        jobserver_auth: None,
        deps_dir: None,
    };
    let out_pm = inv_pm.execute().unwrap();
    assert!(out_pm.success);
}

#[test]
fn test_m8_handle_build_lifecycle_and_diagnostics() {
    let tmp = TempDir::new().unwrap();
    let pkg_dir = tmp.path().join("build-pkg");
    fs::create_dir_all(pkg_dir.join("src")).unwrap();

    let cargo_toml = pkg_dir.join("Cargo.toml");
    fs::write(
        &cargo_toml,
        "[package]\nname = \"build-pkg\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();

    let main_rs = pkg_dir.join("src").join("main.rs");
    fs::write(&main_rs, "fn main() {}\n").unwrap();

    let target_dir = tmp.path().join("target");
    let cache_dir = tmp.path().join("cache");

    let args = BuildArgs {
        manifest_path: Some(cargo_toml.clone()),
        target_dir: Some(target_dir.clone()),
        cache_dir: Some(cache_dir.clone()),
        verbose: 0,
        ..Default::default()
    };

    // 1. Cold build
    let code1 = handle_build(&args);
    assert_eq!(code1, 0);

    // 2. Fast-lock hit
    let code2 = handle_build(&args);
    assert_eq!(code2, 0);

    // 3. Compiler error diagnostics rendering
    fs::write(
        &main_rs,
        "fn main() { compile_error!(\"synthetic test error\"); }\n",
    )
    .unwrap();
    let code_fail = handle_build(&args);
    assert_ne!(code_fail, 0);

    // 4. Missing manifest error
    let args_missing = BuildArgs {
        manifest_path: Some(tmp.path().join("NonExistent.toml")),
        ..Default::default()
    };
    assert_eq!(handle_build(&args_missing), 1);
}
