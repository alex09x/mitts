/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::rust::compiler::RustcInvocation;
use mitts::adapter::rust::types::{BuildMode, PlannedCompilationUnit, RustBuildOptions};
use mitts::adapter::rust::RustAdapter;
use std::fs;

#[test]
fn test_m8_rust_adapter_resolve_manifest_and_target_dir() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let manifest_file = temp_dir.path().join("Cargo.toml");
    fs::write(
        &manifest_file,
        "[package]\nname = \"dummy\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();

    // 1. Explicit manifest path that exists
    let opt = RustBuildOptions {
        manifest_path: Some(manifest_file.clone()),
        ..RustBuildOptions::default()
    };
    let resolved = RustAdapter::resolve_manifest(&opt).expect("resolve manifest");
    assert_eq!(resolved, manifest_file);

    // 2. Explicit manifest that does not exist
    let opt_missing = RustBuildOptions {
        manifest_path: Some(temp_dir.path().join("non_existent_Cargo.toml")),
        ..RustBuildOptions::default()
    };
    assert!(RustAdapter::resolve_manifest(&opt_missing).is_err());

    // 3. Resolve target dir explicit
    let custom_target = temp_dir.path().join("custom_target");
    let opt_target = RustBuildOptions {
        target_dir: Some(custom_target.clone()),
        ..RustBuildOptions::default()
    };
    let res_target = RustAdapter::resolve_target_dir(&manifest_file, &opt_target).unwrap();
    assert_eq!(res_target, custom_target);

    // 4. Resolve target dir default (.mitts/target)
    let res_default = RustAdapter::resolve_target_dir(&manifest_file, &opt).unwrap();
    assert_eq!(res_default, temp_dir.path().join("target").join("mitts"));
}

#[test]
fn test_m8_rust_adapter_detect_toolchain_and_cache() {
    // Should detect toolchain successfully and write/read toolchain cache
    let info = RustAdapter::detect_toolchain().expect("detect toolchain");
    assert_eq!(info.name, "rustc");
    assert!(!info.version.is_empty());

    // Call second time to hit cache
    let cached_info = RustAdapter::detect_toolchain().expect("detect toolchain cached");
    assert_eq!(info.version, cached_info.version);
    assert_eq!(info.target_triple, cached_info.target_triple);
}

#[test]
fn test_m8_compiler_invocation_modes_and_args() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let out_dir = temp_dir.path().join("out");
    fs::create_dir_all(&out_dir).unwrap();

    let src_file = temp_dir.path().join("lib.rs");
    fs::write(&src_file, "pub fn value() -> u32 { 100 }\n").unwrap();

    // Test with pre-release version semver and extra compiler args
    let unit = PlannedCompilationUnit {
        package_id: "demo-pkg 1.2.3-beta.1".to_string(),
        package_name: "demo-pkg".to_string(),
        version: "1.2.3-beta.1".to_string(),
        target_name: "demo_pkg".to_string(),
        kind: "lib".to_string(),
        src_path: src_file,
        manifest_path: temp_dir.path().join("Cargo.toml"),
        cache_eligibility: "eligible".to_string(),
        is_workspace: true,
        env_vars: vec![],
        dependencies: vec![],
        edition: Some("2021".to_string()),
        features: vec![],
    };

    let extra_args = vec!["-C".to_string(), "opt-level=0".to_string()];
    let inv = RustcInvocation {
        unit: &unit,
        action_key: "short_key_123",
        out_dir: &out_dir,
        dest_binary: None,
        extern_crates: &[],
        release: false,
        features: &[],
        directives: None,
        mode: BuildMode::Build,
        extra_compiler_args: &extra_args,
        jobserver_auth: None,
        deps_dir: None,
    };

    let out = inv.execute().expect("execute rustc");
    assert!(out.success, "rustc execution failed: {}", out.stderr);
    assert!(out.primary_output.is_some());

    // Test check mode
    let inv_check = RustcInvocation {
        unit: &unit,
        action_key: "short_key_123",
        out_dir: &out_dir,
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

    let out_check = inv_check.execute().expect("execute check");
    assert!(out_check.success);
}

#[test]
fn test_m8_compiler_non_workspace_cap_lints() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let out_dir = temp_dir.path().join("out_non_ws");
    fs::create_dir_all(&out_dir).unwrap();

    let src_file = temp_dir.path().join("lib.rs");
    // Code with unused variable that would trigger a warning
    fs::write(&src_file, "pub fn foo() { let x = 1; }\n").unwrap();

    let unit = PlannedCompilationUnit {
        package_id: "ext-pkg 0.1.0".to_string(),
        package_name: "ext-pkg".to_string(),
        version: "0.1.0".to_string(),
        target_name: "ext_pkg".to_string(),
        kind: "lib".to_string(),
        src_path: src_file,
        manifest_path: temp_dir.path().join("Cargo.toml"),
        cache_eligibility: "eligible".to_string(),
        is_workspace: false, // Non-workspace crate -> cap-lints allow
        env_vars: vec![],
        dependencies: vec![],
        edition: Some("2021".to_string()),
        features: vec![],
    };

    let inv = RustcInvocation {
        unit: &unit,
        action_key: "non_ws_key_456",
        out_dir: &out_dir,
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

    let out = inv.execute().expect("execute rustc non workspace");
    assert!(out.success);
}
