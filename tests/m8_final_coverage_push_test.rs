/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::fs;
use std::path::Path;
use tempfile::TempDir;

use mitts::adapter::c::{CAdapter, CBuildOptions};
use mitts::adapter::rust::compiler::RustcInvocation;
use mitts::adapter::rust::fetcher::FetchOptions;
use mitts::adapter::rust::inputs::append_unit_dep_info_inputs;
use mitts::adapter::rust::source_lookup::find_crate_source_dir;
use mitts::adapter::rust::types::PlannedCompilationUnit;
use mitts::adapter::rust::{BuildMode, RustAdapter, RustBuildOptions};
use mitts::cli::diagnostic::render_compiler_diagnostics;
use mitts::cli::handlers::handle_gc;
use mitts::cli::test::{handle_test, TestArgs};
use mitts::cli::GcArgs;
use mitts::core::lock::FileLock;
use mitts::core::reflink::reflink_or_copy;
use mitts::core::source_store::SourceStore;

#[test]
fn test_m8_diagnostic_fallback_and_malformed_sections() {
    // 1. Diagnostics empty fallback (unstructured panic/abort)
    render_compiler_diagnostics("fatal: unexpected internal compiler error\n", false);
    render_compiler_diagnostics("aborting due to 1 internal error", true);

    // 2. Split section without trailing quote
    render_compiler_diagnostics(
        "=== Compilation failed for unit malformed_no_quote\nerror: internal issue\n",
        false,
    );
}

#[test]
fn test_m8_adapter_detect_toolchain_cache_and_build_native() {
    let temp = TempDir::new().unwrap();

    // 1. Detect toolchain -> searches PATH and saves/updates cache
    let info1 = RustAdapter::detect_toolchain().unwrap();
    assert!(!info1.name.is_empty());

    // 2. Second detection -> loads from cache (mtime cache hit)
    let info2 = RustAdapter::detect_toolchain().unwrap();
    assert_eq!(info1.target_triple, info2.target_triple);

    // 3. Build native DAG execution on minimal library package
    let pkg_dir = temp.path().join("native_dag_pkg");
    let src_dir = pkg_dir.join("src");
    fs::create_dir_all(&src_dir).unwrap();
    fs::write(
        pkg_dir.join("Cargo.toml"),
        br#"[package]
name = "native_dag_pkg"
version = "0.1.0"
edition = "2021"
"#,
    )
    .unwrap();
    fs::write(src_dir.join("lib.rs"), b"pub fn run() -> u32 { 42 }\n").unwrap();

    let opt = RustBuildOptions {
        manifest_path: Some(pkg_dir.join("Cargo.toml")),
        ..Default::default()
    };
    let (plan, res) = RustAdapter::build_native(&opt).unwrap();
    assert!(!plan.actions.is_empty());
    assert!(res.success);

    // 4. RustAdapter::fetch on local manifest
    let store = SourceStore::new(temp.path().join("store")).unwrap();
    let fetch_res = RustAdapter::fetch(
        &pkg_dir.join("Cargo.toml"),
        &store,
        &FetchOptions {
            offline: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(fetch_res.total_packages, 0);
}

#[test]
fn test_m8_test_cli_failing_binary_and_execution_errors() {
    let temp = TempDir::new().unwrap();
    let mock_pkg = temp.path().join("mock_test_pkg");
    let mock_src = mock_pkg.join("src");
    fs::create_dir_all(&mock_src).unwrap();
    fs::write(
        mock_pkg.join("Cargo.toml"),
        br#"[package]
name = "mock_test_pkg"
version = "0.1.0"
edition = "2021"
"#,
    )
    .unwrap();
    fs::write(
        mock_src.join("lib.rs"),
        b"#[test] fn t_fail() { panic!(\"intentional failure\"); }\n",
    )
    .unwrap();

    // 1. Failing test execution -> handle_test catches non-zero exit and finishes failed
    let args_fail = TestArgs {
        manifest_path: Some(mock_pkg.join("Cargo.toml")),
        ..Default::default()
    };
    let code_fail = handle_test(&args_fail);
    assert_ne!(code_fail, 0);
}

#[test]
fn test_m8_handlers_gc_action_cache_dir_error_and_compiler_dest() {
    let temp = TempDir::new().unwrap();

    // 1. handle_gc where CAS succeeds but AC fails because path is a file
    let bad_cache = temp.path().join("bad_cache");
    fs::create_dir_all(bad_cache.join("cas")).unwrap();
    fs::write(bad_cache.join("ac"), b"not_a_dir").unwrap();

    let gc_args = GcArgs {
        max_bytes: None,
        max_age_secs: None,
        cache_dir: Some(bad_cache),
    };
    let exit_gc = handle_gc(&gc_args);
    assert_eq!(exit_gc, 1);

    // 2. RustcInvocation kind=test with nested dest_binary creating parent directories
    let out_dir = temp.path().join("out");
    fs::create_dir_all(&out_dir).unwrap();
    let src_file = temp.path().join("test_lib.rs");
    fs::write(&src_file, b"#[test] fn t() {}\n").unwrap();

    let unit = PlannedCompilationUnit {
        package_id: "test_dest_pkg@0.1.0".to_string(),
        package_name: "test_dest_pkg".to_string(),
        version: "0.1.0".to_string(),
        target_name: "test_dest_pkg".to_string(),
        kind: "test".to_string(),
        src_path: src_file,
        manifest_path: temp.path().join("Cargo.toml"),
        cache_eligibility: "Uncached".to_string(),
        is_workspace: true,
        is_primary: false,
        env_vars: Vec::new(),
        dependencies: Vec::new(),
        edition: Some("2021".to_string()),
        features: Vec::new(),
        for_host: false,
        workspace_root: None,
    };

    let dest_bin = out_dir.join("nested_bin_dir").join("my_test_bin");
    let inv = RustcInvocation {
        unit: &unit,
        action_key: "short_key_dest_test",
        out_dir: &out_dir,
        dest_binary: Some(&dest_bin),
        extern_crates: &[],
        release: false,
        features: &[],
        directives: None,
        mode: BuildMode::Build,
        extra_compiler_args: &[],
        jobserver_auth: None,
        deps_dir: None,
        target_dir: None,
    };

    let res = inv.execute().unwrap();
    assert!(res.success);
    assert!(dest_bin.exists());
}

#[test]
fn test_m8_reflink_and_c_adapter_and_lock_gap_pushes() {
    let temp = TempDir::new().unwrap();

    // 1. reflink_or_copy destination rename error when destination is an existing directory
    let src = temp.path().join("src.txt");
    fs::write(&src, b"source_payload_bytes").unwrap();
    let dst_dir = temp.path().join("existing_dir");
    fs::create_dir_all(&dst_dir).unwrap();
    let reflink_err = reflink_or_copy(&src, &dst_dir, false);
    assert!(reflink_err.is_err());

    // 2. CAdapter::resolve_manifest in repo root (no Mitts.toml)
    let c_opt = CBuildOptions::default();
    let c_res = CAdapter::resolve_manifest(&c_opt);
    assert!(c_res.is_err());

    // 3. FileLock empty path (no parent directory) error branches
    let lock_err1 = FileLock::acquire_exclusive(Path::new(""));
    assert!(lock_err1.is_err());
    let lock_err2 = FileLock::try_acquire_exclusive(Path::new(""));
    assert!(lock_err2.is_err());
    let lock_err3 = FileLock::acquire_shared(Path::new(""));
    assert!(lock_err3.is_err());

    // 4. RustcInvocation kind=bin with non-existent src_path (testing fallback logic)
    let unit_missing = PlannedCompilationUnit {
        package_id: "missing_src_pkg@0.1.0".to_string(),
        package_name: "missing_src_pkg".to_string(),
        version: "0.1.0".to_string(),
        target_name: "missing_src_pkg".to_string(),
        kind: "bin".to_string(),
        src_path: temp.path().join("does_not_exist_main.rs"),
        manifest_path: temp.path().join("Cargo.toml"),
        cache_eligibility: "Uncached".to_string(),
        is_workspace: true,
        is_primary: false,
        env_vars: Vec::new(),
        dependencies: Vec::new(),
        edition: Some("2021".to_string()),
        features: Vec::new(),
        for_host: false,
        workspace_root: None,
    };
    let out_dir2 = temp.path().join("out2");
    fs::create_dir_all(&out_dir2).unwrap();
    let inv_missing = RustcInvocation {
        unit: &unit_missing,
        action_key: "short_key_missing_src",
        out_dir: &out_dir2,
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
    };
    let res_missing = inv_missing.execute().unwrap();
    assert!(!res_missing.success);

    // 5. find_crate_source_dir with non-git source falling back to git checkouts
    let fake_home = temp.path().join("fake_home_checkout");
    let checkout = fake_home
        .join(".cargo")
        .join("git")
        .join("checkouts")
        .join("repo-xyz")
        .join("commit-456");
    fs::create_dir_all(&checkout).unwrap();
    fs::write(
        checkout.join("Cargo.toml"),
        br#"[package]
name = "non_git_checkout_crate"
version = "2.0.0"
"#,
    )
    .unwrap();
    let orig_home = std::env::var("HOME").ok();
    std::env::set_var("HOME", &fake_home);
    let found = find_crate_source_dir("non_git_checkout_crate", "2.0.0", None);
    assert_eq!(found, Some(checkout));
    if let Some(h) = orig_home {
        std::env::set_var("HOME", h);
    } else {
        std::env::remove_var("HOME");
    }
}

#[test]
fn test_m8_planner_path_dep_injection_and_dep_info_empty_inputs() {
    let temp = TempDir::new().unwrap();

    // 1. Path dependency injection into lockfile + non-workspace bin target + unresolvable lib unit
    let ws_pkg = temp.path().join("ws_planner_pkg");
    let ws_src = ws_pkg.join("src");
    let dep_pkg = temp.path().join("path_sub_dep");
    let dep_src = dep_pkg.join("src");
    fs::create_dir_all(&ws_src).unwrap();
    fs::create_dir_all(&dep_src).unwrap();

    fs::write(
        ws_pkg.join("Cargo.toml"),
        br#"[package]
name = "ws_planner_pkg"
version = "0.1.0"
edition = "2021"

[dependencies]
path_sub_dep = { path = "../path_sub_dep" }
"#,
    )
    .unwrap();
    fs::write(ws_src.join("lib.rs"), b"pub fn f() {}\n").unwrap();

    // path_sub_dep has lib.rs
    fs::write(
        dep_pkg.join("Cargo.toml"),
        br#"[package]
name = "path_sub_dep"
version = "0.2.0"
edition = "2021"
"#,
    )
    .unwrap();
    fs::write(dep_src.join("lib.rs"), b"pub fn dep_func() {}\n").unwrap();

    // Lockfile without path_sub_dep -> triggers path_dependencies.contains_key injection
    fs::write(
        ws_pkg.join("Cargo.lock"),
        br#"version = 3

[[package]]
name = "ws_planner_pkg"
version = "0.1.0"
dependencies = [
    "path_sub_dep",
]
"#,
    )
    .unwrap();

    let ws_opt = RustBuildOptions {
        manifest_path: Some(ws_pkg.join("Cargo.toml")),
        ..Default::default()
    };
    let exp = RustAdapter::explain_plan(&ws_opt).unwrap();
    assert_eq!(exp.units.len(), 2);

    // 2. append_unit_dep_info_inputs where inputs is empty and dep-info contains .rs inside package
    let target_dir = temp.path().join("target");
    let debug_deps = target_dir.join("debug").join("deps");
    fs::create_dir_all(&debug_deps).unwrap();
    let d_file = debug_deps.join("my_target.d");
    let tracked_rs = ws_src.join("lib.rs");
    fs::write(
        &d_file,
        format!(
            "{}: {}\n",
            ws_pkg.join("target/lib.rlib").display(),
            tracked_rs.display()
        ),
    )
    .unwrap();

    let appended =
        append_unit_dep_info_inputs(Vec::new(), &ws_pkg, &target_dir, "my_target").unwrap();
    assert!(!appended.is_empty());
}
