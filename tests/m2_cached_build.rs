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
use mitts::core::action_cache::ActionCache;
use mitts::core::cas::ContentAddressedStore;
use mitts::core::hash::Sha256;
use mitts::core::lock::FileLock;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Barrier};
use std::thread;

fn fixture_manifest() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("hello-rust")
        .join("Cargo.toml")
}

#[test]
fn test_cas_store_and_integrity_verification() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let cas_root = temp_dir.path().join("cas");
    let cas = ContentAddressedStore::new(cas_root.clone()).expect("new CAS");

    // Test storing in-memory bytes
    let payload = b"immutable compiler artifact bytes";
    let (hash, size) = cas.store_bytes(payload).expect("store bytes");
    assert_eq!(size, payload.len() as u64);
    assert_eq!(hash, Sha256::digest(payload));

    let obj_path = cas.object_path(&hash);
    assert!(obj_path.is_file(), "Object file must exist in CAS");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::metadata(&obj_path).expect("metadata").permissions();
        assert_eq!(
            perms.mode() & 0o777,
            0o444,
            "CAS object must be marked read-only (0o444)"
        );
    }

    // Verify integrity
    assert!(cas.verify_object(&hash).expect("verify object"));

    // Restore object as executable
    let dest_bin = temp_dir.path().join("restored_bin");
    cas.restore_object(&hash, &dest_bin, true).expect("restore object");
    assert!(dest_bin.is_file());
    assert_eq!(fs::read(&dest_bin).expect("read restored"), payload);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::metadata(&dest_bin).expect("metadata").permissions();
        assert_eq!(
            perms.mode() & 0o111,
            0o111,
            "Restored executable must have execute permissions"
        );
    }
}

#[test]
fn test_cas_detects_corrupted_object() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let cas = ContentAddressedStore::new(temp_dir.path().join("cas")).expect("new CAS");

    let payload = b"legitimate artifact data";
    let (hash, _) = cas.store_bytes(payload).expect("store bytes");
    let obj_path = cas.object_path(&hash);

    // Tamper with the object file by corrupting bytes
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // Make writable temporarily to tamper
        fs::set_permissions(&obj_path, fs::Permissions::from_mode(0o644)).expect("set perm");
    }
    fs::write(&obj_path, b"corrupted artifact data").expect("tamper with object");

    // Integrity check must fail
    assert!(
        !cas.verify_object(&hash).expect("verify object"),
        "Corrupted object must fail integrity check"
    );

    // Restore must return an error and remove the poisoned file
    let dest = temp_dir.path().join("dest");
    let res = cas.restore_object(&hash, &dest, false);
    assert!(res.is_err(), "Restoring corrupted object must fail");
    assert!(!obj_path.exists(), "Corrupted object must be removed from CAS");
}

#[test]
fn test_action_cache_key_derivation_deterministic() {
    let key1 = ActionCache::derive_action_key(
        "rustc 1.85.0",
        "x86_64-unknown-linux-gnu",
        "debug",
        &["-C".to_string(), "opt-level=0".to_string()],
        &["feature_b".to_string(), "feature_a".to_string()], // permuted features
        &[("src/main.rs", "hash_main"), ("src/lib.rs", "hash_lib")],
        &[("itoa", "1.0.14")],
    );

    let key2 = ActionCache::derive_action_key(
        "rustc 1.85.0",
        "x86_64-unknown-linux-gnu",
        "debug",
        &["-C".to_string(), "opt-level=0".to_string()],
        &["feature_a".to_string(), "feature_b".to_string()], // canonical order
        &[("src/lib.rs", "hash_lib"), ("src/main.rs", "hash_main")], // permuted sources
        &[("itoa", "1.0.14")],
    );

    assert_eq!(
        key1, key2,
        "Action keys must canonicalize feature names and source lists"
    );

    // Order-sensitive compiler flags: permuting them MUST change the key!
    let key_flags_permuted = ActionCache::derive_action_key(
        "rustc 1.85.0",
        "x86_64-unknown-linux-gnu",
        "debug",
        &["opt-level=0".to_string(), "-C".to_string()], // permuted flags
        &["feature_a".to_string(), "feature_b".to_string()],
        &[("src/main.rs", "hash_main"), ("src/lib.rs", "hash_lib")],
        &[("itoa", "1.0.14")],
    );
    assert_ne!(
        key1, key_flags_permuted,
        "Compiler flags are order-sensitive and must not produce colliding keys"
    );

    // Change toolchain version -> key changes
    let key_diff_toolchain = ActionCache::derive_action_key(
        "rustc 1.86.0",
        "x86_64-unknown-linux-gnu",
        "debug",
        &["-C".to_string(), "opt-level=0".to_string()],
        &["default".to_string()],
        &[("src/main.rs", "hash_main")],
        &[],
    );
    assert_ne!(key1, key_diff_toolchain);

    // Change source hash -> key changes
    let key_diff_source = ActionCache::derive_action_key(
        "rustc 1.85.0",
        "x86_64-unknown-linux-gnu",
        "debug",
        &["-C".to_string(), "opt-level=0".to_string()],
        &["default".to_string()],
        &[("src/main.rs", "hash_main_modified")],
        &[("itoa", "1.0.14")],
    );
    assert_ne!(key1, key_diff_source);
}

#[test]
fn test_single_flight_file_lock() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let lock_file = temp_dir.path().join("test.lock");

    let lock1 = FileLock::acquire_exclusive(&lock_file).expect("acquire exclusive");
    assert!(lock1.path().is_file());

    // Try acquire should fail while lock1 is held
    let lock2_attempt = FileLock::try_acquire_exclusive(&lock_file).expect("try acquire");
    assert!(lock2_attempt.is_none(), "Lock must be held exclusively");

    // Drop lock1
    drop(lock1);

    // Try acquire should now succeed
    let lock2 = FileLock::try_acquire_exclusive(&lock_file).expect("try acquire after release");
    assert!(lock2.is_some(), "Lock must be acquirable after release");
}

#[test]
fn test_cold_and_warm_build_with_cas_and_action_cache() {
    let temp_cache = tempfile::tempdir().expect("cache tempdir");
    let temp_target = tempfile::tempdir().expect("target tempdir");
    let target_dir = temp_target.path().to_path_buf();

    let manifest = fixture_manifest();

    // 1. Cold build: must run compiler and populate cache
    let opt_cold = RustBuildOptions {
        manifest_path: Some(manifest.clone()),
        cache_dir: Some(temp_cache.path().to_path_buf()),
        target_dir: Some(target_dir.clone()),
        ..Default::default()
    };
    let (_plan_cold, result_cold) =
        RustAdapter::build(&opt_cold).expect("Cold build must succeed");
    assert!(result_cold.success);
    assert!(!result_cold.cached, "Cold build must not be marked cached");

    let bin_path = target_dir.join("debug").join("hello-rust");
    assert!(bin_path.exists(), "Target binary must exist after cold build");

    // Verify CAS objects were written
    let cas_dir = temp_cache.path().join("cas").join("objects");
    assert!(cas_dir.is_dir(), "CAS objects dir must exist");
    let cas_count = fs::read_dir(&cas_dir).expect("read cas").count();
    assert!(cas_count > 0, "CAS must contain published objects");

    // Verify AC entry was written
    let ac_dir = temp_cache.path().join("ac");
    assert!(ac_dir.is_dir(), "AC dir must exist");
    let ac_count = fs::read_dir(&ac_dir)
        .expect("read ac")
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map(|ext| ext == "json").unwrap_or(false))
        .count();
    assert!(ac_count > 0, "Action Cache must contain entry");

    // 2. Warm build: must hit cache and return cached = true
    let opt_warm = RustBuildOptions {
        manifest_path: Some(manifest.clone()),
        cache_dir: Some(temp_cache.path().to_path_buf()),
        target_dir: Some(target_dir.clone()),
        ..Default::default()
    };
    let (_, result_warm) = RustAdapter::build(&opt_warm).expect("Warm build must succeed");
    assert!(result_warm.success);
    assert!(result_warm.cached, "Warm build must be marked cached");

    // 3. Remove target binary to simulate clean workspace restore from cache
    let _ = fs::remove_file(&bin_path);
    assert!(!bin_path.exists());

    let (_, result_restore) =
        RustAdapter::build(&opt_warm).expect("Warm build restore must succeed");
    assert!(result_restore.success);
    assert!(result_restore.cached, "Restored build must be cached");
    assert!(bin_path.exists(), "Binary must be restored from CAS into target directory");

    // Verify restored binary produces correct observable output
    let run_out = Command::new(&bin_path).output().expect("execute restored binary");
    assert_eq!(run_out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&run_out.stdout);
    assert!(stdout.contains("Hello from Mitts fixture: 42"));

    // 4. Test --no-cache bypass
    let opt_no_cache = RustBuildOptions {
        manifest_path: Some(manifest),
        cache_dir: Some(temp_cache.path().to_path_buf()),
        target_dir: Some(target_dir),
        no_cache: true,
        ..Default::default()
    };
    let (_, result_no_cache) =
        RustAdapter::build(&opt_no_cache).expect("Build with --no-cache must succeed");
    assert!(result_no_cache.success);
    assert!(!result_no_cache.cached, "Build with --no-cache must not be cached");
}

#[test]
fn test_source_modification_invalidates_action_cache() {
    let temp_cache = tempfile::tempdir().expect("cache tempdir");
    let temp_target = tempfile::tempdir().expect("target tempdir");
    let temp_ws = tempfile::tempdir().expect("ws tempdir");
    let ws_path = temp_ws.path();

    // Copy fixture to isolated workspace
    let src_fixture = fixture_manifest().parent().unwrap().to_path_buf();
    copy_dir_recursive(&src_fixture, ws_path).expect("copy fixture");

    let manifest = ws_path.join("Cargo.toml");
    let opt = RustBuildOptions {
        manifest_path: Some(manifest.clone()),
        cache_dir: Some(temp_cache.path().to_path_buf()),
        target_dir: Some(temp_target.path().to_path_buf()),
        ..Default::default()
    };

    // Cold build
    let (_, res1) = RustAdapter::build(&opt).expect("cold build");
    assert!(!res1.cached);

    // Warm build
    let (_, res2) = RustAdapter::build(&opt).expect("warm build");
    assert!(res2.cached);

    // Modify source file
    let main_rs = ws_path.join("src").join("main.rs");
    let original_content = fs::read_to_string(&main_rs).expect("read main.rs");
    let modified_content = original_content.replace("42", "99");
    fs::write(&main_rs, modified_content).expect("write modified main.rs");

    // Build after modification: Action key must change, cache miss!
    let (_, res3) = RustAdapter::build(&opt).expect("modified build");
    assert!(!res3.cached, "Modified source must trigger cache miss");

    // Verify modified output
    let bin = temp_target.path().join("debug").join("hello-rust");
    let run_out = Command::new(&bin).output().expect("execute modified binary");
    let stdout = String::from_utf8_lossy(&run_out.stdout);
    assert!(stdout.contains("Hello from Mitts fixture: 99"));

    // Warm build of modified source
    let (_, res4) = RustAdapter::build(&opt).expect("warm modified build");
    assert!(res4.cached, "Unchanged modified source must hit cache");
}

#[test]
fn test_corrupted_cas_triggers_recompilation() {
    let temp_cache = tempfile::tempdir().expect("cache tempdir");
    let temp_target = tempfile::tempdir().expect("target tempdir");

    let manifest = fixture_manifest();
    let opt = RustBuildOptions {
        manifest_path: Some(manifest),
        cache_dir: Some(temp_cache.path().to_path_buf()),
        target_dir: Some(temp_target.path().to_path_buf()),
        ..Default::default()
    };

    // Cold build
    let (_, res1) = RustAdapter::build(&opt).expect("cold build");
    assert!(!res1.cached);

    // Corrupt all CAS objects
    let cas_dir = temp_cache.path().join("cas").join("objects");
    for entry in fs::read_dir(&cas_dir).expect("read cas").flatten() {
        let p = entry.path();
        if p.is_file() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = fs::set_permissions(&p, fs::Permissions::from_mode(0o644));
            }
            let _ = fs::write(&p, b"corrupted binary artifact");
        }
    }

    // Build again: corrupt entry must be detected, causing a cache miss and clean recompilation
    let (_, res2) = RustAdapter::build(&opt).expect("build after corruption");
    assert!(
        !res2.cached,
        "Corrupted CAS entries must trigger recompilation"
    );
    assert!(res2.success);

    // Following build should hit the repaired cache
    let (_, res3) = RustAdapter::build(&opt).expect("build after repair");
    assert!(res3.cached, "Repaired cache entry must hit");
}

#[test]
fn test_concurrent_builds_coordinate_via_single_flight() {
    let temp_cache = tempfile::tempdir().expect("cache tempdir");
    let temp_target = tempfile::tempdir().expect("target tempdir");
    let target_dir = Arc::new(temp_target.path().to_path_buf());
    let cache_dir = Arc::new(temp_cache.path().to_path_buf());

    let manifest = fixture_manifest();
    let barrier = Arc::new(Barrier::new(4));
    let mut handles = Vec::new();

    for _ in 0..4 {
        let b = Arc::clone(&barrier);
        let m = manifest.clone();
        let td = Arc::clone(&target_dir);
        let cd = Arc::clone(&cache_dir);
        handles.push(thread::spawn(move || {
            b.wait();
            let opt = RustBuildOptions {
                manifest_path: Some(m),
                cache_dir: Some((*cd).clone()),
                target_dir: Some((*td).clone()),
                ..Default::default()
            };
            RustAdapter::build(&opt).expect("Concurrent build must succeed")
        }));
    }

    for h in handles {
        let (_, res) = h.join().expect("join thread");
        assert!(res.success, "All concurrent builds must succeed");
    }
}

#[test]
fn test_per_unit_action_caching_and_dependency_isolation() {
    let temp_cache = tempfile::tempdir().expect("cache tempdir");
    let temp_target = tempfile::tempdir().expect("target tempdir");
    let temp_ws = tempfile::tempdir().expect("ws tempdir");
    let ws_path = temp_ws.path();

    let src_fixture = fixture_manifest().parent().unwrap().to_path_buf();
    copy_dir_recursive(&src_fixture, ws_path).expect("copy fixture");

    let manifest = ws_path.join("Cargo.toml");
    let opt = RustBuildOptions {
        manifest_path: Some(manifest.clone()),
        cache_dir: Some(temp_cache.path().to_path_buf()),
        target_dir: Some(temp_target.path().to_path_buf()),
        ..Default::default()
    };

    // 1. Initial build: populates AC entries for both itoa (dependency) and hello-rust (root)
    let (_, res1) = RustAdapter::build(&opt).expect("initial build");
    assert!(!res1.cached);
    let ac_count = fs::read_dir(temp_cache.path().join("ac"))
        .expect("read ac")
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map(|ext| ext == "json").unwrap_or(false))
        .count();
    assert!(ac_count > 0, "Action Cache must contain published entries");
    let explanation_before = RustAdapter::explain_plan(&opt).expect("explain plan");
    let itoa_unit = explanation_before
        .units
        .iter()
        .find(|u| u.target_name == "itoa")
        .expect("itoa unit");
    assert_eq!(itoa_unit.package_name, "itoa");

    // 2. Modify consumer application source only
    let main_rs = ws_path.join("src").join("main.rs");
    let original = fs::read_to_string(&main_rs).expect("read main");
    fs::write(&main_rs, original.replace("42", "123")).expect("write main");

    // 3. Re-explain plan after consumer modification
    let explanation_after = RustAdapter::explain_plan(&opt).expect("explain plan after");
    let itoa_unit_after = explanation_after
        .units
        .iter()
        .find(|u| u.target_name == "itoa")
        .expect("itoa unit after");

    // Dependency unit inputs, version, and dependencies are identical
    assert_eq!(itoa_unit.dependencies, itoa_unit_after.dependencies);
    assert_eq!(itoa_unit.version, itoa_unit_after.version);

    // 4. Build after consumer modification
    let (_, res2) = RustAdapter::build(&opt).expect("build after edit");
    assert!(!res2.cached, "Consumer modification triggers rebuild");
    assert!(res2.success);

    // 5. Subsequent build with identical inputs hits cache
    let (_, res3) = RustAdapter::build(&opt).expect("warm build");
    assert!(res3.cached, "Subsequent build must hit cache");
}

#[test]
fn test_action_key_incorporates_cargo_config_and_env() {
    let temp_ws = tempfile::tempdir().expect("ws tempdir");
    let ws_path = temp_ws.path();

    let src_fixture = fixture_manifest().parent().unwrap().to_path_buf();
    copy_dir_recursive(&src_fixture, ws_path).expect("copy fixture");

    let manifest = ws_path.join("Cargo.toml");
    let opt1 = RustBuildOptions {
        manifest_path: Some(manifest.clone()),
        ..Default::default()
    };
    let expl1 = RustAdapter::explain_plan(&opt1).expect("explain plan");

    // Modifying .cargo/config.toml in package directory must be captured
    let cargo_dir = ws_path.join(".cargo");
    fs::create_dir_all(&cargo_dir).expect("create .cargo");
    let config_path = cargo_dir.join("config.toml");
    fs::write(&config_path, "[build]\nrustflags = [\"-C\", \"opt-level=1\"]\n")
        .expect("write config.toml");

    let expl2 = RustAdapter::explain_plan(&opt1).expect("explain plan 2");
    assert_eq!(expl1.units.len(), expl2.units.len());
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if src_path.is_dir() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str != "target" && name_str != ".mitts" {
                copy_dir_recursive(&src_path, &dst_path)?;
            }
        } else {
            fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}
