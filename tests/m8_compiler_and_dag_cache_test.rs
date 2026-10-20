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
use mitts::adapter::rust::types::{BuildMode, PlannedCompilationUnit};
use std::fs;

#[test]
fn test_m8_compiler_source_fallback_and_test_dest_none() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    // 1. Source store fallback: unit.src_path does not exist on disk,
    // but $MITTS_CACHE_DIR/sources/<pkg>-<ver>/src/lib.rs exists!
    let store_sources = root
        .join("cache")
        .join("sources")
        .join("cached-dep-0.2.0")
        .join("src");
    fs::create_dir_all(&store_sources).unwrap();
    fs::write(
        store_sources.join("lib.rs"),
        "pub fn cached_val() -> u32 { 42 }\n",
    )
    .unwrap();

    std::env::set_var("MITTS_CACHE_DIR", root.join("cache").display().to_string());

    let out_dir = root.join("out");
    fs::create_dir_all(&out_dir).unwrap();

    let unit = PlannedCompilationUnit {
        package_id: "cached-dep@0.2.0".into(),
        package_name: "cached-dep".into(),
        version: "0.2.0".into(),
        target_name: "cached_dep".into(),
        kind: "lib".into(),
        src_path: root.join("nonexistent_path").join("lib.rs"),
        manifest_path: root.join("Cargo.toml"),
        cache_eligibility: "Immutable".into(),
        is_workspace: false,
        is_primary: false,
        env_vars: Vec::new(),
        dependencies: Vec::new(),
        edition: Some("2021".into()),
        features: Vec::new(),
        for_host: false,
        workspace_root: None,
    };

    let invocation = RustcInvocation {
        unit: &unit,
        action_key: "shortkey1",
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
        target_dir: None,
        cache_root: None,
    };

    let res = invocation.execute();
    assert!(
        res.is_ok(),
        "compiler should discover source in cache: {:?}",
        res.err()
    );
    let out = res.unwrap();
    assert!(out.success);

    // 2. Compilation of test binary with dest_binary = None (covers out_dir fallback for tests)
    let test_src = root.join("test_src");
    fs::create_dir_all(&test_src).unwrap();
    let test_file = test_src.join("my_test.rs");
    fs::write(&test_file, "#[test] fn t() {}\n").unwrap();

    let test_unit = PlannedCompilationUnit {
        package_id: "test-unit@0.1.0".into(),
        package_name: "test-unit".into(),
        version: "0.1.0".into(),
        target_name: "my_test".into(),
        kind: "test".into(),
        src_path: test_file,
        manifest_path: root.join("Cargo.toml"),
        cache_eligibility: "Workspace".into(),
        is_workspace: true,
        is_primary: false,
        env_vars: Vec::new(),
        dependencies: Vec::new(),
        edition: Some("2021".into()),
        features: Vec::new(),
        for_host: false,
        workspace_root: None,
    };

    let test_invocation = RustcInvocation {
        unit: &test_unit,
        action_key: "shortkey2",
        out_dir: &out_dir,
        dest_binary: None, // dest_binary is None
        extern_crates: &[],
        release: false,
        features: &[],
        directives: None,
        mode: BuildMode::Test,
        extra_compiler_args: &[],
        jobserver_auth: None,
        deps_dir: None,
        target_dir: None,
        cache_root: None,
    };

    let test_res = test_invocation.execute();
    assert!(
        test_res.is_ok(),
        "test_res err: {:?}",
        test_res.as_ref().err()
    );
    let out = test_res.unwrap();
    assert!(out.success, "test out stderr: {}", out.stderr);
}
