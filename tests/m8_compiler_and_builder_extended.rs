/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::rust::build_script::BuildScriptDirectives;
use mitts::adapter::rust::compiler::RustcInvocation;
use mitts::adapter::rust::types::{BuildMode, PlannedCompilationUnit, RustBuildOptions};
use mitts::adapter::RustAdapter;
use std::collections::HashMap;
use std::fs;
use tempfile::TempDir;

#[test]
fn test_m8_compiler_check_and_clippy_modes() {
    let tmp = TempDir::new().unwrap();
    let src_dir = tmp.path().join("src");
    let out_dir = tmp.path().join("out");
    fs::create_dir_all(&src_dir).unwrap();
    fs::create_dir_all(&out_dir).unwrap();

    let lib_rs = src_dir.join("lib.rs");
    fs::write(&lib_rs, "pub fn greet() -> &'static str { \"hello\" }\n").unwrap();
    let manifest = tmp.path().join("Cargo.toml");
    fs::write(
        &manifest,
        "[package]\nname = \"check-crate\"\nversion = \"0.1.0-alpha.1\"\n",
    )
    .unwrap();

    let unit = PlannedCompilationUnit {
        package_id: "check-crate@0.1.0-alpha.1".to_string(),
        package_name: "check-crate".to_string(),
        version: "0.1.0-alpha.1".to_string(),
        target_name: "check_crate".to_string(),
        kind: "lib".to_string(),
        src_path: lib_rs.clone(),
        manifest_path: manifest.clone(),
        cache_eligibility: "Workspace".to_string(),
        is_workspace: true,
        is_primary: false,
        env_vars: Vec::new(),
        dependencies: Vec::new(),
        edition: Some("2021".to_string()),
        features: Vec::new(),
    };

    // 1. BuildMode::Check emits rmeta
    let inv_check = RustcInvocation {
        unit: &unit,
        action_key: "check_action_key_1234567890abcdef",
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
    let out_check = inv_check.execute().unwrap();
    assert!(out_check.success);
    assert_eq!(out_check.exit_code, 0);
    assert!(out_check.primary_output.is_some() || out_check.metadata_output.is_some());

    // 2. BuildMode::Check with binary unit
    let main_rs = src_dir.join("main.rs");
    fs::write(&main_rs, "fn main() {}\n").unwrap();
    let mut bin_unit = unit.clone();
    bin_unit.kind = "bin".to_string();
    bin_unit.target_name = "check_bin".to_string();
    bin_unit.src_path = main_rs;
    let inv_bin_check = RustcInvocation {
        unit: &bin_unit,
        action_key: "check_bin_key_1234567890abcdef",
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
    let out_bin = inv_bin_check.execute().unwrap();
    assert!(out_bin.success);

    // 3. BuildMode::Clippy
    let inv_clippy = RustcInvocation {
        unit: &unit,
        action_key: "clippy_action_key_1234567890abcdef",
        out_dir: &out_dir,
        dest_binary: None,
        extern_crates: &[],
        release: true,
        features: &[],
        directives: None,
        mode: BuildMode::Clippy,
        extra_compiler_args: &[],
        jobserver_auth: None,
        deps_dir: None,
    };
    let out_clippy = inv_clippy.execute().unwrap();
    // In environments where clippy-driver is a fallback, rustc succeeds with --emit=dep-info,metadata
    assert!(out_clippy.success);
}

#[test]
fn test_m8_compiler_directives_and_error_handling() {
    let tmp = TempDir::new().unwrap();
    let src_dir = tmp.path().join("src");
    let out_dir = tmp.path().join("out");
    fs::create_dir_all(&src_dir).unwrap();
    fs::create_dir_all(&out_dir).unwrap();

    let lib_rs = src_dir.join("lib.rs");
    fs::write(
        &lib_rs,
        "#[cfg(custom_cfg)] pub const HAS_CFG: bool = true;\npub fn f() {}\n",
    )
    .unwrap();
    let manifest = tmp.path().join("Cargo.toml");
    fs::write(
        &manifest,
        "[package]\nname = \"dir-crate\"\nversion = \"1\"\n",
    )
    .unwrap();

    let unit = PlannedCompilationUnit {
        package_id: "dir-crate@1".to_string(),
        package_name: "dir-crate".to_string(),
        version: "1".to_string(), // non-semver (1 part)
        target_name: "dir_crate".to_string(),
        kind: "lib".to_string(),
        src_path: lib_rs.clone(),
        manifest_path: manifest.clone(),
        cache_eligibility: "Workspace".to_string(),
        is_workspace: false,
        is_primary: false,
        env_vars: Vec::new(),
        dependencies: Vec::new(),
        edition: Some("2021".to_string()),
        features: vec!["foo".to_string()],
    };

    let mut envs = HashMap::new();
    envs.insert("CUSTOM_VAR".to_string(), "val".to_string());
    let directives = BuildScriptDirectives {
        cfgs: vec!["custom_cfg".to_string()],
        link_libs: Vec::new(),
        link_searches: vec![tmp.path().display().to_string()],
        envs,
        out_dir: Some(tmp.path().join("out_build")),
        metadata: HashMap::new(),
        links: None,
        ..Default::default()
    };

    let inv = RustcInvocation {
        unit: &unit,
        action_key: "key_with_directives_1234567890",
        out_dir: &out_dir,
        dest_binary: None,
        extern_crates: &[],
        release: false,
        features: &["foo".to_string()],
        directives: Some(&directives),
        mode: BuildMode::Build,
        extra_compiler_args: &["--verbose".to_string()],
        jobserver_auth: None,
        deps_dir: None,
    };
    let out = inv.execute().unwrap();
    assert!(out.success);
    assert_eq!(out.exit_code, 0);

    // Test failure exit code on invalid rust source
    let bad_src = src_dir.join("bad.rs");
    fs::write(&bad_src, "broken syntax {(").unwrap();
    let mut bad_unit = unit.clone();
    bad_unit.src_path = bad_src;
    let inv_fail = RustcInvocation {
        unit: &bad_unit,
        action_key: "key_fail_123456",
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
    let out_fail = inv_fail.execute().unwrap();
    assert!(!out_fail.success);
    assert_ne!(out_fail.exit_code, 0);
    assert!(!out_fail.stderr.is_empty());
}

#[test]
fn test_m8_builder_unsupported_options_and_modes() {
    let tmp = TempDir::new().unwrap();
    let manifest = tmp.path().join("Cargo.toml");
    let src = tmp.path().join("src");
    fs::create_dir_all(&src).unwrap();
    fs::write(
        &manifest,
        "[package]\nname = \"opt-pkg\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(src.join("lib.rs"), "pub fn a() {}\n").unwrap();

    // 1. Explicitly unsupported option error
    let opt_unsupp = RustBuildOptions {
        manifest_path: Some(manifest.clone()),
        extra_args: vec!["--unsupported-flag".to_string()],
        ..Default::default()
    };
    let res = RustAdapter::build(&opt_unsupp);
    assert!(res.is_err(), "Unsupported option must return error");

    // 2. Normal build in check mode (release and dev)
    let opt_check_rel = RustBuildOptions {
        manifest_path: Some(manifest.clone()),
        release: true,
        mode: BuildMode::Check,
        ..Default::default()
    };
    let (plan, res) = RustAdapter::build(&opt_check_rel).unwrap();
    assert!(res.success);
    assert_eq!(res.exit_code, 0);
    assert!(!plan.actions.is_empty());

    // 3. Fast build lock hit on second build
    let (_plan_fast, res_fast) = RustAdapter::build(&opt_check_rel).unwrap();
    assert!(res_fast.success);
    assert!(res_fast.cached);
    assert_eq!(res_fast.action_id, "fast-lock-hit");
}
