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
use mitts::adapter::rust::dag_cache::{try_restore_from_cache, ExecutionContext};
use mitts::adapter::rust::types::{
    BuildProgressCallback, PlannedCompilationUnit, RustBuildOptions,
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
fn test_m8_dag_cache_missing_cas_object_and_links_metadata() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    let cas = ContentAddressedStore::new(root.join("cas")).unwrap();
    let ac = ActionCache::new(root.join("ac")).unwrap();
    let target_dir = root.join("target");
    fs::create_dir_all(&target_dir).unwrap();

    let manifest_path = root.join("Cargo.toml");
    fs::write(
        &manifest_path,
        r#"
[package]
name = "links-crate"
version = "0.1.0"
links = "native_zlib"
"#,
    )
    .unwrap();

    let unit = PlannedCompilationUnit {
        package_id: "links-crate@0.1.0".to_string(),
        package_name: "links-crate".to_string(),
        version: "0.1.0".to_string(),
        target_name: "links_crate".to_string(),
        kind: "lib".to_string(),
        src_path: root.join("src").join("lib.rs"),
        manifest_path,
        cache_eligibility: "eligible".to_string(),
        is_workspace: true,
        is_primary: false,
        env_vars: Vec::new(),
        dependencies: Vec::new(),
        edition: Some("2021".to_string()),
        features: Vec::new(),
        for_host: false,
        workspace_root: None,
    };

    let opt = RustBuildOptions {
        manifest_path: Some(unit.manifest_path.clone()),
        target_dir: Some(target_dir.clone()),
        native: true,
        ..Default::default()
    };

    let toolchain = ToolchainInfo {
        name: "rustc".to_string(),
        version: "1.80.0".to_string(),
        target_triple: Some("x86_64".to_string()),
    };

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
        completed_cfgs: None,
    };

    let action_key = "test_missing_cas_key";

    // 1. Entry exists in AC, but object is missing in CAS -> valid = false
    let missing_entry = ActionCacheEntry {
        action_key: action_key.to_string(),
        action_id: "links-crate::lib".to_string(),
        created_at_secs: 100,
        outputs: vec![CachedOutput {
            relative_path: PathBuf::from("liblinks_crate.rlib"),
            cas_hash: "0000000000000000000000000000000000000000000000000000000000000000"
                .to_string(),
            file_size: 100,
            executable: false,
        }],
        stdout: String::new(),
        stderr: String::new(),
        exit_code: 0,
    };
    ctx.ac.put(&missing_entry).unwrap();

    let restored = try_restore_from_cache(&unit, action_key, &ctx, false).unwrap();
    assert!(!restored);

    // 2. Real CAS object stored, but links = "native_zlib" without metadata file -> returns Ok(false)
    let (real_hash, real_size) = ctx.cas.store_bytes(b"real rlib bytes").unwrap();
    let valid_cas_entry = ActionCacheEntry {
        action_key: action_key.to_string(),
        action_id: "links-crate::lib".to_string(),
        created_at_secs: 100,
        outputs: vec![CachedOutput {
            relative_path: PathBuf::from("liblinks_crate.rlib"),
            cas_hash: real_hash.clone(),
            file_size: real_size,
            executable: false,
        }],
        stdout: String::new(),
        stderr: "warning: test compiler message\n".to_string(),
        exit_code: 0,
    };
    ctx.ac.put(&valid_cas_entry).unwrap();

    let restored_no_meta = try_restore_from_cache(&unit, action_key, &ctx, false).unwrap();
    assert!(!restored_no_meta);

    // 3. Create metadata file with dep_vars, rmeta output, and progress message
    let meta_dir = ctx.cas.root().join("metadata");
    fs::create_dir_all(&meta_dir).unwrap();
    let mut meta = HashMap::new();
    meta.insert("root".to_string(), "/opt/zlib".to_string());
    let directives = BuildScriptDirectives {
        links: Some("native_zlib".to_string()),
        metadata: meta,
        ..Default::default()
    };
    let toml_data = toml::to_string(&directives).unwrap();
    fs::write(meta_dir.join(format!("{}.toml", action_key)), toml_data).unwrap();

    // Add rmeta output
    let (rmeta_hash, rmeta_size) = ctx.cas.store_bytes(b"rmeta bytes").unwrap();
    let multi_output_entry = ActionCacheEntry {
        action_key: action_key.to_string(),
        action_id: "links-crate::lib".to_string(),
        created_at_secs: 100,
        outputs: vec![
            CachedOutput {
                relative_path: PathBuf::from("liblinks_crate.rlib"),
                cas_hash: real_hash,
                file_size: real_size,
                executable: false,
            },
            CachedOutput {
                relative_path: PathBuf::from("liblinks_crate.rmeta"),
                cas_hash: rmeta_hash,
                file_size: rmeta_size,
                executable: false,
            },
        ],
        stdout: String::new(),
        stderr: "warning: unit built with warning\n".to_string(),
        exit_code: 0,
    };
    ctx.ac.put(&multi_output_entry).unwrap();
    ctx.ac
        .record_manifest_entry(action_key, Vec::new(), action_key)
        .unwrap();

    let progress_called = Arc::new(Mutex::new(false));
    let progress_flag = progress_called.clone();
    let opt_with_cb = RustBuildOptions {
        manifest_path: Some(unit.manifest_path.clone()),
        target_dir: Some(target_dir.clone()),
        native: false, // tests copying into target/debug/deps
        progress: Some(BuildProgressCallback(Arc::new(move |_| {
            *progress_flag.lock().unwrap() = true;
        }))),
        ..Default::default()
    };
    let ctx2 = ExecutionContext {
        opt: &opt_with_cb,
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
        completed_cfgs: None,
    };

    let restored_with_meta = try_restore_from_cache(&unit, action_key, &ctx2, false).unwrap();
    assert!(restored_with_meta);
    assert!(*progress_called.lock().unwrap());
    assert!(dep_build_metadata
        .lock()
        .unwrap()
        .contains_key("links-crate"));
}

#[test]
fn test_m8_dag_cache_binary_restore_copy() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    let cas = ContentAddressedStore::new(root.join("cas")).unwrap();
    let ac = ActionCache::new(root.join("ac")).unwrap();
    let target_dir = root.join("target");
    fs::create_dir_all(&target_dir).unwrap();

    let manifest_path = root.join("Cargo.toml");
    fs::write(
        &manifest_path,
        "[package]\nname = \"bin-pkg\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();

    let unit = PlannedCompilationUnit {
        package_id: "bin-pkg@0.1.0".to_string(),
        package_name: "bin-pkg".to_string(),
        version: "0.1.0".to_string(),
        target_name: "my_bin".to_string(),
        kind: "bin".to_string(),
        src_path: root.join("src").join("main.rs"),
        manifest_path,
        cache_eligibility: "eligible".to_string(),
        is_workspace: true,
        is_primary: false,
        env_vars: Vec::new(),
        dependencies: Vec::new(),
        edition: Some("2021".to_string()),
        features: Vec::new(),
        for_host: false,
        workspace_root: None,
    };

    let opt = RustBuildOptions {
        manifest_path: Some(unit.manifest_path.clone()),
        target_dir: Some(target_dir.clone()),
        native: true,
        ..Default::default()
    };

    let toolchain = ToolchainInfo {
        name: "rustc".to_string(),
        version: "1.80.0".to_string(),
        target_triple: Some("x86_64".to_string()),
    };

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
        completed_cfgs: None,
    };

    let (bin_hash, bin_size) = ctx.cas.store_bytes(b"mock executable content").unwrap();
    let action_key = "bin_action_key_123";
    let entry = ActionCacheEntry {
        action_key: action_key.to_string(),
        action_id: "bin-pkg::bin:my_bin".to_string(),
        created_at_secs: 100,
        outputs: vec![CachedOutput {
            // Relative path different from dest_bin (dest_bin is debug/my_bin)
            relative_path: PathBuf::from("other_dir").join("my_bin"),
            cas_hash: bin_hash,
            file_size: bin_size,
            executable: true,
        }],
        stdout: String::new(),
        stderr: String::new(),
        exit_code: 0,
    };
    ctx.ac.put(&entry).unwrap();
    ctx.ac
        .record_manifest_entry(action_key, Vec::new(), action_key)
        .unwrap();

    let restored = try_restore_from_cache(&unit, action_key, &ctx, true).unwrap();
    assert!(restored);
    let dest_bin = target_dir.join("debug").join("my_bin");
    assert!(dest_bin.is_file());
    assert_eq!(fs::read(&dest_bin).unwrap(), b"mock executable content");
}
