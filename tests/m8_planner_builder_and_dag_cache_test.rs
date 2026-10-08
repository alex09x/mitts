/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::rust::builder::build;
use mitts::adapter::rust::dag_cache::{try_restore_from_cache, ExecutionContext};
use mitts::adapter::rust::types::{
    BuildProgressCallback, BuildProgressEvent, PlannedCompilationUnit, RustBuildOptions,
};
use mitts::cli::handlers::{handle_clean, handle_explain, handle_fetch, handle_gc};
use mitts::cli::{CleanArgs, ExplainArgs, FetchArgs, GcArgs};
use mitts::core::action::ToolchainInfo;
use mitts::core::action_cache::{ActionCache, ActionCacheEntry, CachedOutput};
use mitts::core::cas::ContentAddressedStore;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[test]
fn test_m8_dag_cache_direct_restoration_branches() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    let cas = ContentAddressedStore::new(root.join("cas")).unwrap();
    let ac = ActionCache::new(root.join("ac")).unwrap();
    let target_dir = root.join("target");
    let completed_artifacts = Arc::new(Mutex::new(HashMap::new()));
    let recorded_actions = Arc::new(Mutex::new(Vec::new()));
    let compiler_messages = Arc::new(Mutex::new(Vec::new()));
    let any_missed = Arc::new(Mutex::new(false));
    let dep_build_metadata = Arc::new(Mutex::new(HashMap::new()));

    let toolchain = ToolchainInfo {
        name: "rustc".to_string(),
        version: "1.80.0".to_string(),
        target_triple: Some("x86_64-unknown-linux-gnu".to_string()),
    };

    let opt = RustBuildOptions {
        release: false,
        native: true,
        ..Default::default()
    };

    let manifest_path = root.join("Cargo.toml");
    fs::write(
        &manifest_path,
        r#"[package]
name = "cached_pkg"
version = "0.1.0"
edition = "2021"
"#,
    )
    .unwrap();

    let unit = PlannedCompilationUnit {
        package_id: "cached_pkg@0.1.0".to_string(),
        package_name: "cached_pkg".to_string(),
        version: "0.1.0".to_string(),
        target_name: "cached_bin".to_string(),
        kind: "bin".to_string(),
        src_path: root.join("src/main.rs"),
        manifest_path: manifest_path.clone(),
        cache_eligibility: "Eligible".to_string(),
        is_workspace: true,
        env_vars: Vec::new(),
        dependencies: Vec::new(),
        edition: Some("2021".to_string()),
        features: Vec::new(),
    };

    // 1. is_cache_enabled: false -> returns false
    let ctx_no_cache = ExecutionContext {
        opt: &opt,
        toolchain: &toolchain,
        target_dir: &target_dir,
        cas: &cas,
        ac: &ac,
        is_cache_enabled: false,
        completed_artifacts: &completed_artifacts,
        recorded_actions: &recorded_actions,
        compiler_messages: &compiler_messages,
        any_missed: &any_missed,
        dep_build_metadata: &dep_build_metadata,
        resolved_keys: None,
        jobserver: None,
        is_first_job: None,
        on_metadata_ready: None,
    };
    assert!(!try_restore_from_cache(&unit, "key1", &ctx_no_cache, true).unwrap());

    // 2. Cache miss -> returns false
    let ctx_enabled = ExecutionContext {
        is_cache_enabled: true,
        ..ctx_no_cache
    };
    assert!(!try_restore_from_cache(&unit, "missing_key", &ctx_enabled, true).unwrap());

    // 3. Valid cache hit for binary
    let (cas_hash, size) = cas.store_bytes(b"mock-binary-executable").unwrap();
    let action_key = "hit_action_key_12345678901234567890123456789012";
    let entry = ActionCacheEntry {
        action_key: action_key.to_string(),
        action_id: "cached_pkg::bin".to_string(),
        created_at_secs: 10,
        outputs: vec![CachedOutput {
            relative_path: PathBuf::from("debug/cached_bin"),
            cas_hash: cas_hash.clone(),
            file_size: size,
            executable: true,
        }],
        stdout: String::new(),
        stderr: "warning: test warning\n".to_string(),
        exit_code: 0,
    };
    ac.put(&entry).unwrap();
    ac.record_manifest_entry(action_key, Vec::new(), action_key)
        .unwrap();

    let mut opt_with_cb = opt.clone();
    let hit_count = Arc::new(Mutex::new(0));
    let hit_clone = hit_count.clone();
    opt_with_cb.progress = Some(BuildProgressCallback(Arc::new(move |event| {
        if matches!(event, BuildProgressEvent::UnitFinished) {
            *hit_clone.lock().unwrap() += 1;
        }
    })));

    let ctx_hit = ExecutionContext {
        opt: &opt_with_cb,
        is_cache_enabled: true,
        ..ctx_no_cache
    };
    let restored = try_restore_from_cache(&unit, action_key, &ctx_hit, true).unwrap();
    assert!(restored);
    assert_eq!(*hit_count.lock().unwrap(), 1);
    assert_eq!(compiler_messages.lock().unwrap().len(), 1);

    // 4. Valid cache hit for library with rmeta and rlib
    let lib_unit = PlannedCompilationUnit {
        kind: "lib".to_string(),
        target_name: "cached_lib".to_string(),
        ..unit.clone()
    };
    let lib_key = "lib_action_key_12345678901234567890123456789012";
    let (rlib_hash, rlib_size) = cas.store_bytes(b"mock-rlib-data").unwrap();
    let lib_entry = ActionCacheEntry {
        action_key: lib_key.to_string(),
        action_id: "cached_pkg::lib".to_string(),
        created_at_secs: 10,
        outputs: vec![CachedOutput {
            relative_path: PathBuf::from("debug/libcached_lib.rlib"),
            cas_hash: rlib_hash,
            file_size: rlib_size,
            executable: false,
        }],
        stdout: String::new(),
        stderr: String::new(),
        exit_code: 0,
    };
    ac.put(&lib_entry).unwrap();
    ac.record_manifest_entry(lib_key, Vec::new(), lib_key)
        .unwrap();

    let lib_restored = try_restore_from_cache(&lib_unit, lib_key, &ctx_hit, false).unwrap();
    assert!(lib_restored);
    assert!(completed_artifacts
        .lock()
        .unwrap()
        .contains_key(&lib_unit.unit_id()));
}

#[test]
fn test_m8_builder_unsupported_options_and_handlers_errors() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    // 1. builder::build with unsupported option
    let opt_unsupported = RustBuildOptions {
        manifest_path: Some(root.join("Cargo.toml")),
        extra_args: vec!["--unsupported-flag-x".to_string()],
        ..Default::default()
    };
    assert!(build(&opt_unsupported).is_err());

    // 2. handle_explain with missing manifest
    let explain_err = ExplainArgs {
        manifest_path: Some(root.join("nonexistent/Cargo.toml")),
        ..Default::default()
    };
    assert_eq!(handle_explain(&explain_err), 1);

    // 3. handle_fetch with missing manifest
    let fetch_err = FetchArgs {
        manifest_path: Some(root.join("nonexistent/Cargo.toml")),
        ..Default::default()
    };
    assert_eq!(handle_fetch(&fetch_err), 1);

    // 4. handle_clean with missing manifest and release flag
    let clean_err = CleanArgs {
        manifest_path: Some(root.join("nonexistent/Cargo.toml")),
        release: true,
        ..Default::default()
    };
    assert_eq!(handle_clean(&clean_err), 1);

    // 5. handle_gc with invalid directory
    let gc_args = GcArgs {
        cache_dir: Some(root.join("nonexistent_cache")),
        max_bytes: Some(1024),
        ..Default::default()
    };
    assert_eq!(handle_gc(&gc_args), 0);
}
