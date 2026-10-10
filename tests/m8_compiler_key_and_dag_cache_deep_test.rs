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
use mitts::adapter::rust::dag_cache::{try_restore_from_cache, ExecutionContext};
use mitts::adapter::rust::key::{
    compute_unit_flags, parallel_map_chunks, resolve_effective_target_triple, resolve_unit_pkg_dir,
};
use mitts::adapter::rust::types::{
    BuildMode, BuildProgressCallback, BuildProgressEvent, PlannedCompilationUnit, RustBuildOptions,
};
use mitts::core::action::ToolchainInfo;
use mitts::core::action_cache::{ActionCache, ActionCacheEntry, CachedOutput};
use mitts::core::cas::ContentAddressedStore;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tempfile::TempDir;

#[test]
fn test_m8_key_triples_and_parallel_chunks() {
    let toolchain = ToolchainInfo {
        name: "rustc".to_string(),
        version: "1.82.0".to_string(),
        target_triple: Some("x86_64-apple-darwin".to_string()),
    };

    // 1. --target flag in extra_args
    let opt_arg = RustBuildOptions {
        extra_args: vec!["--target".to_string(), "wasm32-unknown-unknown".to_string()],
        ..Default::default()
    };
    assert_eq!(
        resolve_effective_target_triple(&opt_arg, &toolchain),
        "wasm32-unknown-unknown"
    );

    // 2. --target=value in extra_args
    let opt_eq = RustBuildOptions {
        extra_args: vec!["--target=aarch64-unknown-linux-gnu".to_string()],
        ..Default::default()
    };
    assert_eq!(
        resolve_effective_target_triple(&opt_eq, &toolchain),
        "aarch64-unknown-linux-gnu"
    );

    // 3. CARGO_BUILD_TARGET environment variable
    std::env::set_var("CARGO_BUILD_TARGET", "riscv64gc-unknown-linux-gnu");
    let opt_empty = RustBuildOptions::default();
    assert_eq!(
        resolve_effective_target_triple(&opt_empty, &toolchain),
        "riscv64gc-unknown-linux-gnu"
    );
    std::env::remove_var("CARGO_BUILD_TARGET");

    // 4. Fallback to host target
    assert_eq!(
        resolve_effective_target_triple(&opt_empty, &toolchain),
        "x86_64-apple-darwin"
    );

    // 5. parallel_map_chunks with 0 and 1 items
    let empty: Vec<i32> = parallel_map_chunks(&[], |x: &i32| *x * 2);
    assert!(empty.is_empty());

    let single: Vec<i32> = parallel_map_chunks(&[21], |x: &i32| *x * 2);
    assert_eq!(single, vec![42]);

    // 6. resolve_unit_pkg_dir fallback when no manifest exists
    let unit = PlannedCompilationUnit {
        package_id: "test".to_string(),
        package_name: "test".to_string(),
        version: "1.0.0".to_string(),
        target_name: "test".to_string(),
        kind: "bin".to_string(),
        src_path: PathBuf::from("/nonexistent/orphan/src/main.rs"),
        manifest_path: PathBuf::from("/nonexistent/orphan/MissingCargo.toml"),
        cache_eligibility: "test".to_string(),
        is_workspace: true,
        is_primary: false,
        env_vars: vec!["CUSTOM_SET_VAR".to_string(), "UNSET_VAR_XYZ".to_string()],
        dependencies: Vec::new(),
        edition: None,
        features: Vec::new(),
        for_host: false,
        workspace_root: None,
    };
    let pkg_dir = resolve_unit_pkg_dir(&unit);
    assert!(pkg_dir.ends_with("src"));

    // 7. compute_unit_flags with env vars
    std::env::set_var("CUSTOM_SET_VAR", "present_value");
    let flags = compute_unit_flags(&unit, &pkg_dir, &[], BuildMode::Build).unwrap();
    assert!(flags
        .iter()
        .any(|f| f == "ENV_MACRO:CUSTOM_SET_VAR=present_value"));
    assert!(flags.iter().any(|f| f == "ENV_MACRO:UNSET_VAR_XYZ=<unset>"));
    std::env::remove_var("CUSTOM_SET_VAR");
}

#[test]
fn test_m8_compiler_non_semver_and_invocation() {
    let temp = TempDir::new().unwrap();
    let src_file = temp.path().join("src").join("main.rs");
    fs::create_dir_all(temp.path().join("src")).unwrap();
    fs::write(&src_file, "fn main() {}").unwrap();

    let unit_non_semver = PlannedCompilationUnit {
        package_id: "nonsemver".to_string(),
        package_name: "nonsemver".to_string(),
        version: "2".to_string(), // not major.minor.patch
        target_name: "nonsemver".to_string(),
        kind: "bin".to_string(),
        src_path: src_file.clone(),
        manifest_path: temp.path().join("Cargo.toml"),
        cache_eligibility: "test".to_string(),
        is_workspace: true,
        is_primary: false,
        env_vars: Vec::new(),
        dependencies: Vec::new(),
        edition: Some("2021".to_string()),
        features: vec!["std".to_string()],
        for_host: false,
        workspace_root: None,
    };

    let out_dir = temp.path().join("out");
    fs::create_dir_all(&out_dir).unwrap();

    let inv = RustcInvocation {
        unit: &unit_non_semver,
        action_key: "nonsemver_key",
        out_dir: &out_dir,
        dest_binary: None,
        extern_crates: &[],
        release: true, // test release opt-level=3
        features: &["std".to_string()],
        directives: None,
        mode: BuildMode::Build,
        extra_compiler_args: &[],
        jobserver_auth: None,
        deps_dir: None,
    };

    let res = inv.execute();
    assert!(res.is_ok());
}

#[test]
fn test_m8_dag_cache_progress_and_non_native() {
    let temp = TempDir::new().unwrap();
    let cas = ContentAddressedStore::new(temp.path().join("cas")).unwrap();
    let ac = ActionCache::new(temp.path().join("ac")).unwrap();

    let (hash, size) = cas.store_bytes(b"dummy compiled rlib bytes").unwrap();

    let out = CachedOutput {
        relative_path: PathBuf::from("libmock.rlib"),
        cas_hash: hash,
        file_size: size,
        executable: false,
    };

    let action_key = "test_dag_cache_key";
    ac.put(&ActionCacheEntry {
        action_key: action_key.to_string(),
        action_id: "mock_action".to_string(),
        created_at_secs: 100,
        outputs: vec![out],
        stdout: String::new(),
        stderr: "warning: test compiler warning".to_string(),
        exit_code: 0,
    })
    .unwrap();
    ac.record_manifest_entry(action_key, Vec::new(), action_key)
        .unwrap();

    let messages = Arc::new(Mutex::new(Vec::new()));
    let msg_clone = Arc::clone(&messages);

    let progress_cb = BuildProgressCallback(Arc::new(move |event| {
        if let BuildProgressEvent::CompilerMessage(msg) = event {
            msg_clone.lock().unwrap().push(msg);
        }
    }));

    let opt = RustBuildOptions {
        native: false, // test non-native copy to target/deps
        progress: Some(progress_cb),
        ..Default::default()
    };

    let toolchain = ToolchainInfo {
        name: "rustc".to_string(),
        version: "1.82.0".to_string(),
        target_triple: None,
    };

    let target_dir = temp.path().join("target");
    let completed_artifacts = Arc::new(Mutex::new(HashMap::new()));
    let recorded_actions = Arc::new(Mutex::new(Vec::new()));
    let compiler_messages = Arc::new(Mutex::new(Vec::new()));
    let any_missed = Arc::new(Mutex::new(false));
    let dep_build_metadata = Arc::new(Mutex::new(HashMap::new()));

    let ctx = ExecutionContext {
        opt: &opt,
        toolchain: &toolchain,
        target_dir: &target_dir,
        cas: &cas,
        ac: &ac,
        is_cache_enabled: true,
        completed_artifacts: &completed_artifacts,
        recorded_actions: &recorded_actions,
        compiler_messages: &compiler_messages,
        any_missed: &any_missed,
        dep_build_metadata: &dep_build_metadata,
        resolved_keys: None,
        jobserver: None,
        is_first_job: None,
        on_metadata_ready: None,
        compiled_actions: None,
        cached_actions: None,
    };

    let unit = PlannedCompilationUnit {
        package_id: "ws_crate".to_string(),
        package_name: "ws_crate".to_string(),
        version: "0.1.0".to_string(),
        target_name: "ws_crate".to_string(),
        kind: "lib".to_string(),
        src_path: temp.path().join("src/lib.rs"),
        manifest_path: temp.path().join("Cargo.toml"),
        cache_eligibility: "test".to_string(),
        is_workspace: true,
        is_primary: false,
        env_vars: Vec::new(),
        dependencies: Vec::new(),
        edition: None,
        features: Vec::new(),
        for_host: false,
        workspace_root: None,
    };

    let restored = try_restore_from_cache(&unit, action_key, &ctx, false).unwrap();
    assert!(restored);

    // Verify progress callback received the stderr message
    let msgs = messages.lock().unwrap();
    assert!(!msgs.is_empty());
    assert!(msgs[0].contains("warning: test compiler warning"));

    // Verify non-native rlib was placed in target_dir/debug/deps
    assert!(target_dir
        .join("debug")
        .join("deps")
        .join("libmock.rlib")
        .is_file());
}
