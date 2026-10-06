/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::{CAdapter, CBuildOptions, RustAdapter, RustBuildOptions};
use mitts::core::{ActionCache, CacheGc, ContentAddressedStore, GcOptions};
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn c_fixture_manifest() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("hello-c")
        .join("Mitts.toml")
}

fn rust_fixture_manifest() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("hello-rust")
        .join("Cargo.toml")
}

#[test]
fn test_m6_c_manifest_parsing() {
    let manifest_toml = r#"
        [package]
        name = "test-c-pkg"
        version = "1.2.3"

        [bin]
        src = "src/custom_main.c"
    "#;

    let parsed = CAdapter::parse_manifest(manifest_toml).expect("parse manifest");
    assert_eq!(parsed.package.name, "test-c-pkg");
    assert_eq!(parsed.package.version, "1.2.3");
    let bin = parsed.bin.expect("has bin");
    assert_eq!(bin.src, "src/custom_main.c");

    // Test missing package name
    let invalid = r#"
        [bin]
        src = "src/main.c"
    "#;
    assert!(CAdapter::parse_manifest(invalid).is_err());
}

#[test]
fn test_m6_c_adapter_toolchain_detection() {
    let toolchain = CAdapter::detect_toolchain().expect("detect C toolchain");
    assert_eq!(toolchain.name, "cc");
    assert!(!toolchain.version.is_empty(), "Version must not be empty");
}

#[test]
fn test_m6_c_adapter_cold_and_warm_build() {
    let manifest = c_fixture_manifest();
    let temp_target = tempfile::tempdir().expect("target tempdir");
    let temp_cache = tempfile::tempdir().expect("cache tempdir");

    let opt = CBuildOptions {
        manifest_path: Some(manifest),
        target_dir: Some(temp_target.path().to_path_buf()),
        cache_dir: Some(temp_cache.path().to_path_buf()),
        release: false,
        no_cache: false,
        extra_flags: Vec::new(),
    };

    // 1. Cold build
    let (plan, result) = CAdapter::build(&opt).expect("cold build");
    assert!(result.success, "Cold build must succeed");
    assert!(!result.cached, "Cold build must not be marked as cached");
    assert_eq!(plan.actions.len(), 1);

    let bin_path = temp_target.path().join("debug").join("hello-c");
    assert!(
        bin_path.is_file(),
        "Compiled binary must exist at {:?}",
        bin_path
    );

    // Verify binary execution
    let run_out = Command::new(&bin_path).output().expect("execute C binary");
    assert!(run_out.status.success());
    let stdout = String::from_utf8_lossy(&run_out.stdout);
    assert!(
        stdout.contains("Hello from Mitts C fixture: 42"),
        "Unexpected output: {}",
        stdout
    );

    // 2. Warm build (cache hit)
    let (warm_plan, warm_result) = CAdapter::build(&opt).expect("warm build");
    assert!(warm_result.success, "Warm build must succeed");
    assert!(warm_result.cached, "Warm build must be a cache hit");
    assert!(
        warm_plan.actions[0].description.contains("Restored"),
        "Action description should indicate restoration"
    );

    // 3. Delete binary from target dir and rebuild — should restore from CAS
    fs::remove_file(&bin_path).expect("remove binary");
    assert!(!bin_path.exists());

    let (_, restored_result) = CAdapter::build(&opt).expect("restoration build");
    assert!(restored_result.cached, "Must hit cache");
    assert!(bin_path.is_file(), "Binary must be restored from CAS");

    let rerun_out = Command::new(&bin_path)
        .output()
        .expect("execute restored binary");
    assert!(rerun_out.status.success());
    let rerun_stdout = String::from_utf8_lossy(&rerun_out.stdout);
    assert!(rerun_stdout.contains("Hello from Mitts C fixture: 42"));
}

#[test]
fn test_m6_c_adapter_cache_invalidation_on_flags() {
    let manifest = c_fixture_manifest();
    let temp_target = tempfile::tempdir().expect("target tempdir");
    let temp_cache = tempfile::tempdir().expect("cache tempdir");

    let opt_base = CBuildOptions {
        manifest_path: Some(manifest.clone()),
        target_dir: Some(temp_target.path().join("base")),
        cache_dir: Some(temp_cache.path().to_path_buf()),
        release: false,
        no_cache: false,
        extra_flags: Vec::new(),
    };
    let (_, res1) = CAdapter::build(&opt_base).expect("base build");
    assert!(!res1.cached);

    // Build with extra preprocessor flag -> changes ActionKey
    let opt_modified = CBuildOptions {
        manifest_path: Some(manifest),
        target_dir: Some(temp_target.path().join("mod")),
        cache_dir: Some(temp_cache.path().to_path_buf()),
        release: false,
        no_cache: false,
        extra_flags: vec!["-DMITTS_CUSTOM_FLAG=1".to_string()],
    };
    let (_, res2) = CAdapter::build(&opt_modified).expect("modified build");
    assert!(!res2.cached, "Changed flags must produce a cache miss");
}

#[test]
fn test_m6_multi_language_shared_cas_and_action_cache() {
    let shared_cache_dir = tempfile::tempdir().expect("shared cache tempdir");
    let c_target = tempfile::tempdir().expect("c target dir");
    let rust_target = tempfile::tempdir().expect("rust target dir");

    let cas = ContentAddressedStore::new(shared_cache_dir.path().join("cas")).expect("open CAS");
    let ac = ActionCache::new(shared_cache_dir.path().join("ac")).expect("open AC");

    // 1. Build C project into the shared cache
    let c_opt = CBuildOptions {
        manifest_path: Some(c_fixture_manifest()),
        target_dir: Some(c_target.path().to_path_buf()),
        cache_dir: Some(shared_cache_dir.path().to_path_buf()),
        release: false,
        no_cache: false,
        extra_flags: Vec::new(),
    };
    let (_, c_res) = CAdapter::build(&c_opt).expect("C build");
    assert!(c_res.success);
    assert!(!c_res.cached);

    let cas_objects = || -> Vec<String> {
        let mut objs = Vec::new();
        if let Ok(entries) = fs::read_dir(cas.root().join("objects")) {
            for entry in entries.flatten() {
                if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                    objs.push(entry.file_name().to_string_lossy().to_string());
                }
            }
        }
        objs
    };

    let initial_cas_count = cas_objects().len();
    assert!(
        initial_cas_count >= 1,
        "CAS must contain at least the C binary artifact"
    );

    // 2. Build Rust project using the EXACT same shared cache
    let rust_opt = RustBuildOptions {
        manifest_path: Some(rust_fixture_manifest()),
        target_dir: Some(rust_target.path().to_path_buf()),
        cache_dir: Some(shared_cache_dir.path().to_path_buf()),
        release: false,
        native: true,
        ..Default::default()
    };
    let (_, rust_res) = RustAdapter::build_native(&rust_opt).expect("Rust native build");
    assert!(rust_res.success);

    let combined_cas_objects = cas_objects();
    assert!(
        combined_cas_objects.len() > initial_cas_count,
        "CAS must now hold both C and Rust objects"
    );

    // 3. Verify all objects in shared CAS are structurally valid and uncorrupted
    for obj_hash in &combined_cas_objects {
        assert!(
            cas.verify_object(obj_hash).unwrap_or(false),
            "Shared CAS object {} must be verified and uncorrupted",
            obj_hash
        );
    }

    // 4. Rebuild C project — must be a cache hit even with Rust artifacts present
    let (_, c_res2) = CAdapter::build(&c_opt).expect("C rebuild");
    assert!(
        c_res2.cached,
        "C build must hit cache cleanly in shared store"
    );

    // 5. Test unified Garbage Collection across both languages
    let gc = CacheGc::new(&cas, &ac);
    let gc_report = gc
        .prune(&GcOptions {
            max_bytes: None,
            max_age: None,
        })
        .expect("run GC prune");
    assert_eq!(
        gc_report.objects_pruned, 0,
        "Live multi-language artifacts must not be pruned"
    );
}
