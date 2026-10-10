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
use mitts::adapter::rust::compiler::{resolve_clippy_driver, RustcInvocation};
use mitts::adapter::rust::types::{BuildMode, PlannedCompilationUnit};
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

#[test]
fn test_m8_resolve_clippy_driver_variants() {
    let custom = "/tmp/mock-clippy-driver-bin";
    std::env::set_var("CLIPPY_DRIVER", custom);
    let resolved = resolve_clippy_driver();
    assert_eq!(resolved, PathBuf::from(custom));
    std::env::remove_var("CLIPPY_DRIVER");

    let fallback = resolve_clippy_driver();
    assert!(fallback.to_str().unwrap().contains("clippy-driver"));
}

#[test]
fn test_m8_compiler_proc_macro_and_check_modes() {
    let tmp = TempDir::new().unwrap();
    let src_dir = tmp.path().join("src");
    fs::create_dir_all(&src_dir).unwrap();
    let lib_rs = src_dir.join("lib.rs");
    fs::write(
        &lib_rs,
        "extern crate proc_macro; use proc_macro::TokenStream; #[proc_macro] pub fn make_fn(_: TokenStream) -> TokenStream { TokenStream::new() }",
    )
    .unwrap();

    let out_dir = tmp.path().join("out");
    fs::create_dir_all(&out_dir).unwrap();

    let unit = PlannedCompilationUnit {
        package_id: "test-proc@0.1.0-alpha.1".to_string(),
        package_name: "test-proc".to_string(),
        version: "0.1.0-alpha.1".to_string(),
        target_name: "test-proc".to_string(),
        kind: "proc-macro".to_string(),
        src_path: lib_rs.clone(),
        manifest_path: tmp.path().join("Cargo.toml"),
        dependencies: Vec::new(),
        features: vec!["custom-feat".to_string()],
        env_vars: Vec::new(),
        cache_eligibility: "eligible".to_string(),
        is_workspace: true,
        is_primary: false,
        edition: Some("2021".to_string()),
        for_host: false,
    };

    let mut envs = std::collections::HashMap::new();
    envs.insert("CUSTOM_KEY".to_string(), "CUSTOM_VAL".to_string());
    let directives = BuildScriptDirectives {
        out_dir: Some(tmp.path().join("out_dir")),
        cfgs: vec!["mitts_test_cfg".to_string()],
        link_libs: vec!["c".to_string()],
        link_searches: vec![tmp.path().to_string_lossy().to_string()],
        envs,
        ..Default::default()
    };

    let invocation = RustcInvocation {
        unit: &unit,
        action_key: "0123456789abcdef0123456789abcdef",
        out_dir: &out_dir,
        dest_binary: None,
        extern_crates: &[],
        release: true,
        features: &unit.features,
        directives: Some(&directives),
        mode: BuildMode::Build,
        extra_compiler_args: &["-v".to_string()],
        jobserver_auth: None,
        deps_dir: None,
    };

    assert!(invocation.release);
    let res = invocation.execute().unwrap();
    assert!(res.command_line.contains("--crate-type"));
    assert!(res.command_line.contains("proc-macro"));
    assert!(res.command_line.contains("opt-level=3"));
    assert!(res.command_line.contains("mitts_test_cfg"));
}

#[test]
fn test_m8_compiler_workspace_check_mode_and_nonsemver() {
    let tmp = TempDir::new().unwrap();
    let src_dir = tmp.path().join("src");
    fs::create_dir_all(&src_dir).unwrap();
    let main_rs = src_dir.join("main.rs");
    fs::write(&main_rs, "fn main() { println!(\"check\"); }").unwrap();

    let out_dir = tmp.path().join("out");
    fs::create_dir_all(&out_dir).unwrap();

    let unit = PlannedCompilationUnit {
        package_id: "test-check@custom-non-semver".to_string(),
        package_name: "test-check".to_string(),
        version: "custom-non-semver".to_string(),
        target_name: "test-check".to_string(),
        kind: "bin".to_string(),
        src_path: main_rs,
        manifest_path: tmp.path().join("Cargo.toml"),
        dependencies: Vec::new(),
        features: Vec::new(),
        env_vars: Vec::new(),
        cache_eligibility: "eligible".to_string(),
        is_workspace: true,
        is_primary: false,
        edition: None,
        for_host: false,
    };

    let invocation = RustcInvocation {
        unit: &unit,
        action_key: "shortkey",
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

    let res = invocation.execute().unwrap();
    assert!(res.success);
    assert!(res.command_line.contains("--emit=dep-info,metadata"));
    assert!(res.primary_output.is_some() || res.metadata_output.is_some());
}

#[test]
fn test_m8_compiler_non_workspace_cap_lints() {
    let tmp = TempDir::new().unwrap();
    let src_dir = tmp.path().join("src");
    fs::create_dir_all(&src_dir).unwrap();
    let lib_rs = src_dir.join("lib.rs");
    fs::write(&lib_rs, "pub fn helper() {}").unwrap();

    let out_dir = tmp.path().join("out");
    fs::create_dir_all(&out_dir).unwrap();

    let unit = PlannedCompilationUnit {
        package_id: "dep-lib@1.2.3".to_string(),
        package_name: "dep-lib".to_string(),
        version: "1.2.3".to_string(),
        target_name: "dep-lib".to_string(),
        kind: "lib".to_string(),
        src_path: lib_rs,
        manifest_path: tmp.path().join("Cargo.toml"),
        dependencies: Vec::new(),
        features: Vec::new(),
        env_vars: Vec::new(),
        cache_eligibility: "eligible".to_string(),
        is_workspace: false,
        is_primary: false,
        edition: Some("2021".to_string()),
        for_host: false,
    };

    let invocation = RustcInvocation {
        unit: &unit,
        action_key: "depkey1234567890",
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

    let res = invocation.execute().unwrap();
    assert!(res.success);
    assert!(res.command_line.contains("--cap-lints"));
    assert!(res.command_line.contains("allow"));
    assert!(res.primary_output.is_some());
}
