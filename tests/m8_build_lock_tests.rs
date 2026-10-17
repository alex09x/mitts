/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::core::build_lock::{
    FastBuildLock, FreshnessCheckArgs, FreshnessResult, RecordStateArgs,
};
use std::fs;
use std::path::PathBuf;

#[test]
fn test_m8_build_lock_missing_and_corrupt() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let p = temp_dir.path();

    let args = FreshnessCheckArgs {
        project_dir: p,
        profile: "dev",
        package: &None,
        features: &[],
        no_default_features: false,
        all_features: false,
        toolchain_version: "rustc 1.80.0",
    };

    // 1. Missing lock
    let res = FastBuildLock::check_freshness(&args);
    match res {
        FreshnessResult::Stale(msg) => assert!(msg.contains("No build.lock found")),
        _ => panic!("Expected missing lock stale"),
    }

    // 2. Corrupt lock
    let lock_p = FastBuildLock::lock_path(p, "dev");
    fs::create_dir_all(lock_p.parent().unwrap()).unwrap();
    fs::write(&lock_p, "invalid json {[[[").unwrap();

    let res = FastBuildLock::check_freshness(&args);
    match res {
        FreshnessResult::Stale(msg) => assert!(msg.contains("Failed to parse build.lock")),
        _ => panic!("Expected corrupt lock stale"),
    }
}

#[test]
fn test_m8_build_lock_profile_package_feature_toolchain_mismatch() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let p = temp_dir.path();
    let lock_p = FastBuildLock::lock_path(p, "dev");
    fs::create_dir_all(lock_p.parent().unwrap()).unwrap();

    let base_lock = FastBuildLock {
        version: 1,
        profile: "dev".to_string(),
        package: None,
        features: vec!["foo".to_string()],
        no_default_features: false,
        all_features: false,
        toolchain_version: "rustc 1.80.0".to_string(),
        options_hash: None,
        has_complete_inputs: true,
        rustflags: None,
        env_hash: None,
        created_at_secs: 1000,
        cargo_lock: None,
        manifests: vec![],
        sources: vec![],
        directories: vec![],
        cargo_configs: vec![],
        clippy_configs: vec![],
        tracked_extra_files: std::collections::BTreeMap::new(),
        tracked_env_vars: std::collections::BTreeMap::new(),
        outputs: vec![],
        stderr: None,
        action_count: 5,
        root_binary: Some(PathBuf::from("target/dev/app")),
    };

    fs::write(&lock_p, serde_json::to_vec(&base_lock).unwrap()).unwrap();

    // Profile mismatch in lock
    let mut bad_lock = base_lock.clone();
    bad_lock.profile = "release".to_string();
    fs::write(&lock_p, serde_json::to_vec(&bad_lock).unwrap()).unwrap();
    let args = FreshnessCheckArgs {
        project_dir: p,
        profile: "dev",
        package: &None,
        features: &["foo".to_string()],
        no_default_features: false,
        all_features: false,
        toolchain_version: "rustc 1.80.0",
    };
    match FastBuildLock::check_freshness(&args) {
        FreshnessResult::Stale(msg) => assert!(msg.contains("Profile mismatch")),
        _ => panic!("Expected profile mismatch"),
    }

    // Package mismatch
    fs::write(&lock_p, serde_json::to_vec(&base_lock).unwrap()).unwrap();
    let args_pkg = FreshnessCheckArgs {
        package: &Some("pkg-a".to_string()),
        ..args
    };
    match FastBuildLock::check_freshness(&args_pkg) {
        FreshnessResult::Stale(msg) => assert!(msg.contains("Target package mismatch")),
        _ => panic!("Expected target package mismatch"),
    }

    // Feature mismatch
    let args_feat = FreshnessCheckArgs {
        features: &["bar".to_string()],
        ..args
    };
    match FastBuildLock::check_freshness(&args_feat) {
        FreshnessResult::Stale(msg) => assert!(msg.contains("Feature configuration changed")),
        _ => panic!("Expected feature mismatch"),
    }

    // Toolchain mismatch
    let args_tc = FreshnessCheckArgs {
        toolchain_version: "rustc 1.81.0",
        ..args
    };
    match FastBuildLock::check_freshness(&args_tc) {
        FreshnessResult::Stale(msg) => assert!(msg.contains("Toolchain version changed")),
        _ => panic!("Expected toolchain mismatch"),
    }
}

#[test]
fn test_m8_build_lock_files_and_record_state() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let p = temp_dir.path();

    let cargo_toml = p.join("Cargo.toml");
    fs::write(&cargo_toml, "[package]\nname=\"foo\"\nversion=\"0.1.0\"\n").unwrap();

    let cargo_lock = p.join("Cargo.lock");
    fs::write(&cargo_lock, "# lockfile\n").unwrap();

    let src_dir = p.join("src");
    fs::create_dir_all(&src_dir).unwrap();
    let src_file = src_dir.join("main.rs");
    fs::write(&src_file, "fn main() {}\n").unwrap();

    let target_dir = p.join("target").join("dev");
    fs::create_dir_all(&target_dir).unwrap();
    let out_file = target_dir.join("foo");
    fs::write(&out_file, "binary data").unwrap();

    // Record state
    let rec_args = RecordStateArgs {
        project_dir: p,
        profile: "dev",
        package: &None,
        features: &[],
        no_default_features: false,
        all_features: false,
        toolchain_version: "rustc 1.80.0",
        action_count: 1,
        root_binary: Some(PathBuf::from("target/dev/foo")),
        outputs: std::slice::from_ref(&out_file),
        stderr: None,
    };

    FastBuildLock::record_state(&rec_args).expect("record state");

    let check_args = FreshnessCheckArgs {
        project_dir: p,
        profile: "dev",
        package: &None,
        features: &[],
        no_default_features: false,
        all_features: false,
        toolchain_version: "rustc 1.80.0",
    };

    // Should be Fresh
    match FastBuildLock::check_freshness(&check_args) {
        FreshnessResult::Fresh {
            action_count,
            root_binary,
            ..
        } => {
            assert_eq!(action_count, 1);
            assert_eq!(root_binary, Some(PathBuf::from("target/dev/foo")));
        }
        FreshnessResult::Stale(e) => panic!("Expected fresh, got stale: {}", e),
    }

    // Source modified
    fs::write(&src_file, "fn main() { println!(); }\n").unwrap();
    match FastBuildLock::check_freshness(&check_args) {
        FreshnessResult::Stale(msg) => assert!(msg.contains("Source modified")),
        _ => panic!("Expected source modified"),
    }

    // Output truncated
    fs::write(&src_file, "fn main() {}\n").unwrap();
    fs::write(&out_file, "").unwrap();
    match FastBuildLock::check_freshness(&check_args) {
        FreshnessResult::Stale(msg) => {
            assert!(msg.contains("Output artifact truncated") || msg.contains("Source modified"))
        }
        _ => panic!("Expected output truncated or modified"),
    }

    // Output missing
    fs::remove_file(&out_file).unwrap();
    match FastBuildLock::check_freshness(&check_args) {
        FreshnessResult::Stale(msg) => {
            assert!(msg.contains("Output artifact missing") || msg.contains("Source modified"))
        }
        _ => panic!("Expected output missing"),
    }
}

#[test]
fn test_fast_build_lock_warning_replay() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let p = temp_dir.path();
    let src_dir = p.join("src");
    fs::create_dir_all(&src_dir).unwrap();
    let src_file = src_dir.join("main.rs");
    fs::write(&src_file, "fn main() {}\n").unwrap();

    let target_dir = p.join("target").join("check");
    fs::create_dir_all(&target_dir).unwrap();
    let out_file = target_dir.join("libfoo.rmeta");
    fs::write(&out_file, "rmeta data").unwrap();

    let warning_msg = "warning: unused variable: `test_unused`\n --> src/main.rs:1:5";
    let rec_args = RecordStateArgs {
        project_dir: p,
        profile: "check",
        package: &None,
        features: &[],
        no_default_features: false,
        all_features: false,
        toolchain_version: "rustc 1.80.0",
        action_count: 1,
        root_binary: None,
        outputs: std::slice::from_ref(&out_file),
        stderr: Some(warning_msg),
    };

    FastBuildLock::record_state(&rec_args).expect("record state");

    let check_args = FreshnessCheckArgs {
        project_dir: p,
        profile: "check",
        package: &None,
        features: &[],
        no_default_features: false,
        all_features: false,
        toolchain_version: "rustc 1.80.0",
    };

    match FastBuildLock::check_freshness(&check_args) {
        FreshnessResult::Fresh { stderr, .. } => {
            assert_eq!(stderr.as_deref(), Some(warning_msg));
        }
        FreshnessResult::Stale(e) => panic!("Expected fresh with replayed stderr, got: {}", e),
    }
}
