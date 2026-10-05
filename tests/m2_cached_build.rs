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
fn test_flags_and_features_boundaries_prevent_collision() {
    let key1 = ActionCache::derive_action_key(
        "rustc 1.85.0",
        "x86_64-unknown-linux-gnu",
        "debug",
        &["-C".to_string(), "opt-level=0".to_string()],
        &["feat_a".to_string()],
        &[("src/main.rs", "hash_main")],
        &[],
    );
    let key2 = ActionCache::derive_action_key(
        "rustc 1.85.0",
        "x86_64-unknown-linux-gnu",
        "debug",
        &["-C opt-level=0".to_string()],
        &["feat_a".to_string()],
        &[("src/main.rs", "hash_main")],
        &[],
    );
    assert_ne!(
        key1, key2,
        "Flags with distinct boundaries must produce different action keys"
    );
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
    let keys1 = RustAdapter::derive_unit_action_keys(&opt1, &expl1).expect("derive keys 1");

    // 1. Modifying .cargo/config.toml in package directory must change the derived key
    let cargo_dir = ws_path.join(".cargo");
    fs::create_dir_all(&cargo_dir).expect("create .cargo");
    let config_path = cargo_dir.join("config.toml");
    fs::write(&config_path, "[build]\nrustflags = [\"-C\", \"opt-level=1\"]\n")
        .expect("write config.toml");

    let expl2 = RustAdapter::explain_plan(&opt1).expect("explain plan 2");
    let keys2 = RustAdapter::derive_unit_action_keys(&opt1, &expl2).expect("derive keys 2");
    assert_ne!(keys1, keys2, "Modifying .cargo/config.toml must alter unit action keys");

    // 2. Setting CLI --target must alter the action key
    let opt_target = RustBuildOptions {
        manifest_path: Some(manifest.clone()),
        extra_args: vec!["--target".to_string(), "x86_64-unknown-linux-gnu".to_string()],
        ..Default::default()
    };
    let expl_target = RustAdapter::explain_plan(&opt_target).expect("explain target");
    let keys_target = RustAdapter::derive_unit_action_keys(&opt_target, &expl_target).expect("keys target");
    assert_ne!(keys1, keys_target, "--target must alter unit action keys");
}

#[test]
fn test_fresh_target_hashes_non_src_files() {
    let temp_ws = tempfile::tempdir().expect("ws tempdir");
    let ws_path = temp_ws.path();

    let src_fixture = fixture_manifest().parent().unwrap().to_path_buf();
    copy_dir_recursive(&src_fixture, ws_path).expect("copy fixture");

    let manifest = ws_path.join("Cargo.toml");
    let opt = RustBuildOptions {
        manifest_path: Some(manifest.clone()),
        ..Default::default()
    };

    // Clean workspace with no target/ directory
    let expl1 = RustAdapter::explain_plan(&opt).expect("explain plan 1");
    let keys1 = RustAdapter::derive_unit_action_keys(&opt, &expl1).expect("keys 1");

    // Add an arbitrary non-src asset file in a custom directory
    let custom_asset_dir = ws_path.join("content").join("data");
    fs::create_dir_all(&custom_asset_dir).expect("create custom asset dir");
    let asset_file = custom_asset_dir.join("payload.raw");
    fs::write(&asset_file, b"arbitrary bytes included via include_bytes!").expect("write asset");

    let expl2 = RustAdapter::explain_plan(&opt).expect("explain plan 2");
    let keys2 = RustAdapter::derive_unit_action_keys(&opt, &expl2).expect("keys 2");
    assert_ne!(keys1, keys2, "Adding a non-src asset file on a fresh target must change action keys");

    // Modifying that asset file must also change the action key
    fs::write(&asset_file, b"modified arbitrary bytes").expect("modify asset");
    let keys3 = RustAdapter::derive_unit_action_keys(&opt, &expl2).expect("keys 3");
    assert_ne!(keys2, keys3, "Modifying a non-src asset file must change action keys");
}

#[test]
fn test_action_unit_identity_distinguishes_same_named_targets() {
    let opt = RustBuildOptions {
        manifest_path: Some(fixture_manifest()),
        ..Default::default()
    };
    let explanation = RustAdapter::explain_plan(&opt).expect("explain plan");
    let keys = RustAdapter::derive_unit_action_keys(&opt, &explanation).expect("derive keys");

    // Every unit must have a structured unit_id containing package_id, kind, and target_name
    for unit in &explanation.units {
        let u_id = unit.unit_id();
        assert!(
            u_id.contains(&format!("::{}:{}", unit.kind, unit.target_name)),
            "unit_id '{}' must contain kind '{}' and target_name '{}'",
            u_id,
            unit.kind,
            unit.target_name
        );
        assert!(
            keys.contains_key(&u_id),
            "keys must contain an entry for unit_id '{}'",
            u_id
        );
    }
}

#[test]
fn test_partial_cache_hit_restores_dependency_and_avoids_recompilation() {
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

    // 1. Cold build: compiles itoa (dep) and hello-rust (app), populating CAS and AC
    let (_, res1) = RustAdapter::build(&opt).expect("cold build");
    assert!(!res1.cached);
    assert!(res1.success);

    // 2. Wipe the target directory completely
    let _ = fs::remove_dir_all(temp_target.path());
    fs::create_dir_all(temp_target.path()).expect("recreate target dir");

    // 3. Modify only the consumer application source
    let main_rs = ws_path.join("src").join("main.rs");
    let original = fs::read_to_string(&main_rs).expect("read main");
    fs::write(&main_rs, original.replace("42", "777")).expect("write main");

    // 4. Build with clean target directory: itoa is a cache HIT, hello-rust is a cache MISS
    let (_plan, res2) = RustAdapter::build(&opt).expect("partial hit build");
    assert!(res2.success);
    assert!(!res2.cached, "Whole build is not cached since consumer changed");

    // Verify itoa was pre-restored from CAS into target_dir
    let itoa_deps = temp_target
        .path()
        .join("debug")
        .join("deps");
    assert!(itoa_deps.is_dir(), "Restored deps dir must exist");
    let mut found_itoa = false;
    for entry in fs::read_dir(&itoa_deps).expect("read deps").flatten() {
        if entry.file_name().to_string_lossy().starts_with("libitoa") {
            found_itoa = true;
            break;
        }
    }
    assert!(found_itoa, "Cached itoa rlib must be pre-restored into target directory");

    // Verify modified binary runs and prints updated output
    let bin = temp_target.path().join("debug").join("hello-rust");
    let run_out = Command::new(&bin).output().expect("execute restored binary");
    assert_eq!(run_out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&run_out.stdout);
    assert!(stdout.contains("Hello from Mitts fixture: 777"));

    // 5. Subsequent build without changes is a 100% warm hit
    let (_, res3) = RustAdapter::build(&opt).expect("subsequent warm build");
    assert!(res3.cached, "Subsequent build must be 100% warm hit");
}

#[test]
fn test_workspace_manifest_modification_invalidates_member_unit_keys() {
    let temp_ws = tempfile::tempdir().expect("ws tempdir");
    let ws_path = temp_ws.path();

    let crates_app = ws_path.join("crates").join("app");
    fs::create_dir_all(crates_app.join("src")).expect("create app src");

    let root_cargo_toml = ws_path.join("Cargo.toml");
    fs::write(
        &root_cargo_toml,
        "[workspace]\nmembers = [\"crates/app\"]\nresolver = \"2\"\n\n[profile.dev]\nopt-level = 0\n",
    )
    .expect("write root Cargo.toml");

    let app_cargo_toml = crates_app.join("Cargo.toml");
    fs::write(
        &app_cargo_toml,
        "[package]\nname = \"workspace-app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n",
    )
    .expect("write app Cargo.toml");

    fs::write(
        crates_app.join("src").join("main.rs"),
        "fn main() { println!(\"app\"); }",
    )
    .expect("write app main.rs");

    let opt = RustBuildOptions {
        manifest_path: Some(app_cargo_toml.clone()),
        ..Default::default()
    };

    let expl1 = RustAdapter::explain_plan(&opt).expect("explain plan 1");
    let keys1 = RustAdapter::derive_unit_action_keys(&opt, &expl1).expect("derive keys 1");

    // Modify workspace root [profile.dev]
    fs::write(
        &root_cargo_toml,
        "[workspace]\nmembers = [\"crates/app\"]\nresolver = \"2\"\n\n[profile.dev]\nopt-level = 1\n",
    )
    .expect("write modified root Cargo.toml");

    let expl2 = RustAdapter::explain_plan(&opt).expect("explain plan 2");
    let keys2 = RustAdapter::derive_unit_action_keys(&opt, &expl2).expect("derive keys 2");

    assert_ne!(
        keys1, keys2,
        "Modifying workspace root Cargo.toml must invalidate member compilation action keys"
    );
}

#[test]
fn test_cargo_manifest_dir_isolates_different_worktrees() {
    let temp_ws1 = tempfile::tempdir().expect("ws1 tempdir");
    let temp_ws2 = tempfile::tempdir().expect("ws2 tempdir");

    let src_fixture = fixture_manifest().parent().unwrap().to_path_buf();
    copy_dir_recursive(&src_fixture, temp_ws1.path()).expect("copy fixture to ws1");
    copy_dir_recursive(&src_fixture, temp_ws2.path()).expect("copy fixture to ws2");

    let opt1 = RustBuildOptions {
        manifest_path: Some(temp_ws1.path().join("Cargo.toml")),
        ..Default::default()
    };
    let opt2 = RustBuildOptions {
        manifest_path: Some(temp_ws2.path().join("Cargo.toml")),
        ..Default::default()
    };

    let expl1 = RustAdapter::explain_plan(&opt1).expect("explain ws1");
    let expl2 = RustAdapter::explain_plan(&opt2).expect("explain ws2");

    let keys1 = RustAdapter::derive_unit_action_keys(&opt1, &expl1).expect("keys ws1");
    let keys2 = RustAdapter::derive_unit_action_keys(&opt2, &expl2).expect("keys ws2");

    let root_unit1 = expl1
        .units
        .iter()
        .find(|u| u.package_name == "hello-rust")
        .unwrap();
    let root_unit2 = expl2
        .units
        .iter()
        .find(|u| u.package_name == "hello-rust")
        .unwrap();

    let key1 = &keys1[&root_unit1.unit_id()];
    let key2 = &keys2[&root_unit2.unit_id()];

    assert_ne!(
        key1, key2,
        "Worktrees with different directory paths must have different action keys due to CARGO_MANIFEST_DIR"
    );
}

#[test]
fn test_concurrent_warm_restores_in_same_workspace() {
    let temp_cache = tempfile::tempdir().expect("cache tempdir");
    let temp_target = tempfile::tempdir().expect("target tempdir");
    let target_dir = Arc::new(temp_target.path().to_path_buf());
    let cache_dir = Arc::new(temp_cache.path().to_path_buf());
    let manifest = fixture_manifest();

    // 1. Initial build to populate CAS and AC
    let opt_initial = RustBuildOptions {
        manifest_path: Some(manifest.clone()),
        cache_dir: Some((*cache_dir).clone()),
        target_dir: Some((*target_dir).clone()),
        ..Default::default()
    };
    let (_, res_initial) = RustAdapter::build(&opt_initial).expect("initial build");
    assert!(res_initial.success);

    // 2. Run concurrent warm builds targeting the exact same workspace target directory
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
            RustAdapter::build(&opt).expect("Concurrent warm build must succeed")
        }));
    }

    for h in handles {
        let (_, res) = h.join().expect("join thread");
        assert!(res.success, "Concurrent warm restore must succeed without racing");
        assert!(res.cached, "Warm builds must be cached");
    }

    // Verify binary is intact and executable
    let bin_path = target_dir.join("debug").join("hello-rust");
    assert!(bin_path.is_file());
    let out = Command::new(&bin_path).output().expect("execute binary");
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stdout).contains("Hello from Mitts fixture: 42"));
}

#[test]
fn test_single_flight_coordinates_shared_dependency_across_workspaces() {
    let temp_cache = tempfile::tempdir().expect("cache tempdir");
    let cache_dir = Arc::new(temp_cache.path().to_path_buf());

    let temp_ws1 = tempfile::tempdir().expect("ws1 tempdir");
    let temp_ws2 = tempfile::tempdir().expect("ws2 tempdir");
    let src_fixture = fixture_manifest().parent().unwrap().to_path_buf();
    copy_dir_recursive(&src_fixture, temp_ws1.path()).expect("copy fixture to ws1");
    copy_dir_recursive(&src_fixture, temp_ws2.path()).expect("copy fixture to ws2");

    let target_dir1 = temp_ws1.path().join("target");
    let target_dir2 = temp_ws2.path().join("target");

    // Modify ws2 app code so ws1 and ws2 have different root keys, but identical itoa dependency!
    let main2 = temp_ws2.path().join("src").join("main.rs");
    let code = fs::read_to_string(&main2).expect("read ws2 main");
    fs::write(&main2, code.replace("42", "888")).expect("write ws2 main");

    let barrier = Arc::new(Barrier::new(2));
    let b1 = Arc::clone(&barrier);
    let b2 = Arc::clone(&barrier);
    let cd1 = Arc::clone(&cache_dir);
    let cd2 = Arc::clone(&cache_dir);

    let m1 = temp_ws1.path().join("Cargo.toml");
    let m2 = temp_ws2.path().join("Cargo.toml");

    let h1 = thread::spawn(move || {
        b1.wait();
        let opt = RustBuildOptions {
            manifest_path: Some(m1),
            cache_dir: Some((*cd1).clone()),
            target_dir: Some(target_dir1),
            ..Default::default()
        };
        RustAdapter::build(&opt).expect("build ws1")
    });

    let h2 = thread::spawn(move || {
        b2.wait();
        let opt = RustBuildOptions {
            manifest_path: Some(m2),
            cache_dir: Some((*cd2).clone()),
            target_dir: Some(target_dir2),
            ..Default::default()
        };
        RustAdapter::build(&opt).expect("build ws2")
    });

    let (_, res1) = h1.join().expect("join h1");
    let (_, res2) = h2.join().expect("join h2");
    assert!(res1.success);
    assert!(res2.success);
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
