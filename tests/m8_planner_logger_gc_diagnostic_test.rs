/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::rust::resolver::select_best_version;
use mitts::adapter::rust::sparse_index::SparseIndexEntry;
use mitts::adapter::rust::{RustAdapter, RustBuildOptions};
use mitts::cli::diagnostic::render_compiler_diagnostics;
use mitts::core::action_cache::{ActionCache, ActionCacheEntry, CachedOutput};
use mitts::core::build_logger::BuildLogger;
use mitts::core::cas::ContentAddressedStore;
use mitts::core::gc::{CacheGc, GcOptions};
use mitts::core::lock::FileLock;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;
use tempfile::TempDir;

#[test]
fn test_m8_toolchain_caching_and_repeated_calls() {
    let info1 = RustAdapter::detect_toolchain().unwrap();
    assert_eq!(info1.name, "rustc");
    assert!(!info1.version.is_empty());

    // Second call reads from disk cache
    let info2 = RustAdapter::detect_toolchain().unwrap();
    assert_eq!(info1.name, info2.name);
    assert_eq!(info1.version, info2.version);
    assert_eq!(info1.target_triple, info2.target_triple);
}

#[test]
fn test_m8_compiler_diagnostics_unstructured_and_windows_paths() {
    // 1. Unstructured raw stderr without miette spans
    render_compiler_diagnostics(
        "fatal: unexpected compiler panic without structured spans",
        false,
    );
    render_compiler_diagnostics(
        "fatal: unexpected compiler panic without structured spans",
        true,
    );

    // 2. Windows drive letter formatted paths in compiler error spans
    let text = "error[E0308]: mismatched types\n --> C:\\projects\\mitts\\src\\main.rs:42:10\n = help: consider borrowing here";
    render_compiler_diagnostics(text, false);
    render_compiler_diagnostics(text, true);
}

#[test]
fn test_m8_build_logger_file_flushing_and_durations() {
    let temp = TempDir::new().unwrap();
    let log_path = temp.path().join("nested").join("execution.log");

    let mut logger = BuildLogger::new(Some(log_path.clone()));
    logger.record_phase("resolve", Duration::from_millis(25), "resolved 10 packages");
    logger.record_phase("compile", Duration::from_millis(75), "compiled 5 targets");
    logger
        .write_trace("dev", 0, "Build completed successfully")
        .unwrap();

    assert!(log_path.is_file());
    let content = fs::read_to_string(&log_path).unwrap();
    assert!(content.contains("=== Mitts Build Execution Trace ==="));
    assert!(content.contains("resolve"));
    assert!(content.contains("compile"));
    assert!(content.contains("Build completed successfully"));
}

#[test]
fn test_m8_gc_active_lock_protection_and_ac_pruning() {
    let temp = TempDir::new().unwrap();
    let cas = ContentAddressedStore::new(temp.path().join("cas")).unwrap();
    let ac = ActionCache::new(temp.path().join("ac")).unwrap();

    // 1. Store an object and create an action cache entry
    let (cas_hash, size) = cas.store_bytes(b"gc-lock-test-payload").unwrap();
    let entry = ActionCacheEntry {
        action_key: "active_unit_action_key".to_string(),
        action_id: "active_unit_action_key".to_string(),
        created_at_secs: 1234567,
        outputs: vec![CachedOutput {
            relative_path: PathBuf::from("libfoo.rlib"),
            cas_hash: cas_hash.clone(),
            file_size: size,
            executable: false,
        }],
        stdout: String::new(),
        stderr: String::new(),
        exit_code: 0,
    };
    ac.put(&entry).unwrap();

    // 2. Lock the action key's lock file exclusively
    let lock_path = ac.locks_dir().join("active_unit_action_key.lock");
    let held_lock = FileLock::acquire_exclusive(&lock_path).unwrap();

    let gc = CacheGc::new(&cas, &ac);
    let opts = GcOptions {
        max_bytes: Some(0),
        max_age: None,
    };

    // While locked, prune must protect the active CAS object
    let r1 = gc.prune(&opts).unwrap();
    assert_eq!(r1.objects_pruned, 0);

    // 3. Drop lock and prune again — object is pruned, and stale AC entry is cleaned up
    drop(held_lock);
    let r2 = gc.prune(&opts).unwrap();
    assert_eq!(r2.objects_pruned, 1);
    assert_eq!(r2.bytes_freed, size);

    // Stale AC entry referencing pruned hash was removed
    assert!(ac.get("active_unit_action_key").unwrap().is_none());
}

#[test]
fn test_m8_planner_cdylib_eligibility_and_version_selection() {
    let isolated_cache = tempfile::tempdir().expect("isolated cache dir");
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    fs::write(
        root.join("Cargo.toml"),
        r#"
[package]
name = "native_pkg"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib", "rlib"]
path = "src/lib.rs"
"#,
    )
    .unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/lib.rs"), "pub extern \"C\" fn foo() {}").unwrap();

    let opt = RustBuildOptions {
        cache_dir: Some(isolated_cache.path().to_path_buf()),
        manifest_path: Some(root.join("Cargo.toml")),
        ..Default::default()
    };
    let planned = RustAdapter::explain_plan(&opt).unwrap();
    assert_eq!(planned.units.len(), 1);
    assert!(planned.units[0]
        .cache_eligibility
        .contains("native/dynamic library"));

    // Resolver fallback for non-strict semver
    let entries = vec![
        SparseIndexEntry {
            name: "foo".to_string(),
            vers: "0.1.0".to_string(),
            deps: vec![],
            cksum: "abc".to_string(),
            features: Default::default(),
            yanked: false,
            features2: Default::default(),
        },
        SparseIndexEntry {
            name: "foo".to_string(),
            vers: "0.2.0".to_string(),
            deps: vec![],
            cksum: "def".to_string(),
            features: Default::default(),
            yanked: true,
            features2: Default::default(),
        },
    ];
    let chosen = select_best_version(&entries, "invalid-requirement-pattern").unwrap();
    assert_eq!(chosen.vers, "0.1.0");
}

#[test]
fn test_m8_planner_versioned_dep_spec_and_missing_dep() {
    let isolated_cache = tempfile::tempdir().expect("isolated cache dir");
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    fs::write(
        root.join("Cargo.toml"),
        r#"
[package]
name = "parent_pkg"
version = "0.1.0"
edition = "2021"

[dependencies]
child_lib = { path = "child" }

[[bin]]
name = "parent_bin"
path = "src/main.rs"
"#,
    )
    .unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();

    let child_dir = root.join("child");
    fs::create_dir_all(child_dir.join("src")).unwrap();
    fs::write(
        child_dir.join("Cargo.toml"),
        r#"
[package]
name = "child_lib"
version = "0.2.0"
edition = "2021"

[lib]
path = "src/lib.rs"
"#,
    )
    .unwrap();
    fs::write(child_dir.join("src/lib.rs"), "pub fn child() {}").unwrap();

    fs::write(
        root.join("Cargo.lock"),
        r#"
version = 3

[[package]]
name = "parent_pkg"
version = "0.1.0"
dependencies = [
    "child_lib 0.2.0",
    "nonexistent_dep 1.0.0",
]

[[package]]
name = "child_lib"
version = "0.2.0"
"#,
    )
    .unwrap();

    let opt = RustBuildOptions {
        cache_dir: Some(isolated_cache.path().to_path_buf()),
        manifest_path: Some(root.join("Cargo.toml")),
        ..Default::default()
    };
    let planned = RustAdapter::explain_plan(&opt).unwrap();
    assert!(!planned.units.is_empty());
}
