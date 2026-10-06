/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::{RustAdapter, RustBuildOptions};
use mitts::core::{ActionCache, CacheGc, ContentAddressedStore, FileLock, GcOptions};
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

fn fixture_manifest() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("hello-rust")
        .join("Cargo.toml")
}

#[test]
fn test_m5_coexistence_side_by_side_with_cargo() {
    let manifest = fixture_manifest();
    let project_dir = manifest.parent().unwrap();
    let temp_cache = tempfile::tempdir().expect("cache tempdir");

    // 1. Run standard `cargo build` in the fixture directory
    let cargo_status = Command::new("cargo")
        .arg("build")
        .arg("--manifest-path")
        .arg(&manifest)
        .status()
        .expect("invoke cargo build");
    assert!(cargo_status.success(), "Standard cargo build must succeed");

    let cargo_bin = project_dir.join("target").join("debug").join("hello-rust");
    assert!(cargo_bin.is_file(), "Cargo binary must exist in target/debug");

    // 2. Run `mitts build` side-by-side on the exact same project
    let opt = RustBuildOptions {
        manifest_path: Some(manifest.clone()),
        cache_dir: Some(temp_cache.path().to_path_buf()),
        native: true,
        ..Default::default()
    };
    let (plan, result) = RustAdapter::build_native(&opt).expect("Mitts build must succeed");
    assert!(result.success, "Mitts build must succeed: {}", result.stderr);

    let mitts_bin = project_dir
        .join(".mitts")
        .join("target")
        .join("debug")
        .join("hello-rust");
    assert!(mitts_bin.is_file(), "Mitts binary must exist in .mitts/target/debug");

    // 3. Verify both binaries execute and produce expected output
    let cargo_out = Command::new(&cargo_bin).output().expect("run cargo bin");
    assert_eq!(cargo_out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&cargo_out.stdout).contains("Hello from Mitts fixture: 42"));

    let mitts_out = Command::new(&mitts_bin).output().expect("run mitts bin");
    assert_eq!(mitts_out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&mitts_out.stdout).contains("Hello from Mitts fixture: 42"));

    // 4. Verify cargo target directory was NOT corrupted or modified by mitts
    assert!(cargo_bin.is_file());
    assert!(!plan.target_directory.starts_with(project_dir.join("target")));
}

#[test]
fn test_m5_physical_disk_deduplication_across_worktrees() {
    let temp_cache = tempfile::tempdir().expect("shared cache tempdir");
    let shared_cache_dir = temp_cache.path().to_path_buf();

    let temp_wt1 = tempfile::tempdir().expect("worktree 1");
    let temp_wt2 = tempfile::tempdir().expect("worktree 2");

    let wt1_manifest = temp_wt1.path().join("Cargo.toml");
    let wt2_manifest = temp_wt2.path().join("Cargo.toml");

    fs::create_dir_all(temp_wt1.path().join("src")).unwrap();
    fs::create_dir_all(temp_wt2.path().join("src")).unwrap();

    let cargo_content = r#"
[package]
name = "wt-app"
version = "0.1.0"
edition = "2021"

[dependencies]
itoa = "1.0.14"
"#;

    fs::write(&wt1_manifest, cargo_content).unwrap();
    fs::write(&wt2_manifest, cargo_content).unwrap();

    fs::write(
        temp_wt1.path().join("src").join("main.rs"),
        "fn main() { println!(\"wt1: {}\", itoa::Buffer::new().format(1)); }",
    )
    .unwrap();
    fs::write(
        temp_wt2.path().join("src").join("main.rs"),
        "fn main() { println!(\"wt2: {}\", itoa::Buffer::new().format(2)); }",
    )
    .unwrap();

    // Copy Cargo.lock from fixture to avoid needing cargo sync
    let fixture_lock = fixture_manifest().parent().unwrap().join("Cargo.lock");
    fs::copy(&fixture_lock, temp_wt1.path().join("Cargo.lock")).unwrap();
    fs::copy(&fixture_lock, temp_wt2.path().join("Cargo.lock")).unwrap();

    // Build worktree 1 with Mitts
    let opt1 = RustBuildOptions {
        manifest_path: Some(wt1_manifest),
        cache_dir: Some(shared_cache_dir.clone()),
        target_dir: Some(temp_wt1.path().join("target")),
        native: true,
        ..Default::default()
    };
    let (_, res1) = RustAdapter::build_native(&opt1).expect("wt1 build");
    assert!(res1.success);

    // Build worktree 2 with Mitts sharing the same CAS
    let opt2 = RustBuildOptions {
        manifest_path: Some(wt2_manifest),
        cache_dir: Some(shared_cache_dir.clone()),
        target_dir: Some(temp_wt2.path().join("target")),
        native: true,
        ..Default::default()
    };
    let (_, res2) = RustAdapter::build_native(&opt2).expect("wt2 build");
    assert!(res2.success);

    // Verify itoa was stored ONCE in the shared CAS artifacts directory
    let cas_artifacts = shared_cache_dir.join("cas").join("artifacts");
    let itoa_rlibs: Vec<_> = fs::read_dir(&cas_artifacts)
        .expect("read cas artifacts")
        .filter_map(|e| e.ok())
        .filter(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            name.starts_with("libitoa") && name.ends_with(".rlib")
        })
        .collect();

    assert_eq!(
        itoa_rlibs.len(),
        1,
        "Dependency itoa rlib must be stored exactly once in shared CAS across multiple worktrees"
    );

    // Verify neither worktree target has intermediate dependency rlibs
    assert!(!temp_wt1.path().join("target").join("debug").join("deps").join("libitoa.rlib").is_file());
    assert!(!temp_wt2.path().join("target").join("debug").join("deps").join("libitoa.rlib").is_file());
}

#[test]
fn test_m5_benchmarks_cold_vs_warm_speedup() {
    let temp_cache = tempfile::tempdir().expect("cache tempdir");
    let temp_target = tempfile::tempdir().expect("target tempdir");
    let manifest = fixture_manifest();

    let opt = RustBuildOptions {
        manifest_path: Some(manifest),
        cache_dir: Some(temp_cache.path().to_path_buf()),
        target_dir: Some(temp_target.path().to_path_buf()),
        native: true,
        ..Default::default()
    };

    // Cold build timing
    let start_cold = Instant::now();
    let (_, res_cold) = RustAdapter::build_native(&opt).expect("Cold build");
    let duration_cold = start_cold.elapsed();

    assert!(res_cold.success);
    assert!(!res_cold.cached);

    // Warm build timing (100% action cache hit)
    let start_warm = Instant::now();
    let (_, res_warm) = RustAdapter::build_native(&opt).expect("Warm build");
    let duration_warm = start_warm.elapsed();

    assert!(res_warm.success);
    assert!(res_warm.cached, "Warm build must hit action cache");

    println!(
        "Benchmark: Cold = {:?}, Warm = {:?} (Speedup: {:.1}x)",
        duration_cold,
        duration_warm,
        duration_cold.as_secs_f64() / duration_warm.as_secs_f64().max(0.001)
    );

    // Warm build should be substantially faster than cold build
    assert!(
        duration_warm < duration_cold || duration_warm < Duration::from_millis(500),
        "Warm build must achieve fast cache reuse"
    );
}

#[test]
fn test_m5_conservative_gc_and_storage_quotas() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let cas = ContentAddressedStore::new(temp_dir.path().join("cas")).unwrap();
    let ac = ActionCache::new(temp_dir.path().join("ac")).unwrap();

    // Store 3 objects of 1000 bytes each
    let obj1 = vec![1u8; 1000];
    let obj2 = vec![2u8; 1000];
    let obj3 = vec![3u8; 1000];

    let (hash1, _) = cas.store_bytes(&obj1).unwrap();
    std::thread::sleep(Duration::from_millis(20));
    let (hash2, _) = cas.store_bytes(&obj2).unwrap();
    std::thread::sleep(Duration::from_millis(20));
    let (hash3, _) = cas.store_bytes(&obj3).unwrap();

    let gc = CacheGc::new(&cas, &ac);

    // 1. Quota-based pruning: set quota to 2500 bytes (should prune oldest obj1)
    let quota_opts = GcOptions {
        max_bytes: Some(2500),
        max_age: None,
    };

    let report = gc.prune(&quota_opts).expect("prune quota");
    assert_eq!(report.objects_examined, 3);
    assert_eq!(report.objects_pruned, 1, "Should prune exactly 1 object to meet quota");
    assert_eq!(report.bytes_freed, 1000);
    assert_eq!(report.bytes_retained, 2000);

    assert!(!cas.has_object(&hash1), "Oldest object 1 must be pruned");
    assert!(cas.has_object(&hash2), "Object 2 must be retained");
    assert!(cas.has_object(&hash3), "Object 3 must be retained");

    // 2. Active build protection: acquire lock on hash3's key
    let lock_path = ac.lock_path("active_key_3");
    let _active_lock = FileLock::acquire_exclusive(&lock_path).unwrap();

    // Create AC entry referencing hash3 under active_key_3
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let entry = mitts::core::ActionCacheEntry {
        action_key: "active_key_3".to_string(),
        action_id: "active_action".to_string(),
        created_at_secs: now,
        outputs: vec![mitts::core::CachedOutput {
            relative_path: PathBuf::from("dummy"),
            cas_hash: hash3.clone(),
            file_size: 1000,
            executable: false,
        }],
        stdout: String::new(),
        stderr: String::new(),
        exit_code: 0,
    };
    ac.put(&entry).unwrap();

    // Run aggressive prune with quota = 500 bytes: hash3 is protected, so only hash2 should be pruned!
    let aggressive_opts = GcOptions {
        max_bytes: Some(500),
        max_age: None,
    };
    let report2 = gc.prune(&aggressive_opts).expect("aggressive prune");
    assert_eq!(report2.objects_pruned, 1, "Only hash2 should be pruned");
    assert!(!cas.has_object(&hash2), "Hash2 should be pruned");
    assert!(cas.has_object(&hash3), "Active build hash3 must be protected from GC");
}
