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
use mitts::core::lock::FileLock;
use std::fs;
use tempfile::TempDir;

#[test]
fn test_m8_build_lock_stale_branches() {
    let tmp = TempDir::new().unwrap();
    let p = tmp.path();

    // Setup initial project structure
    let src_dir = p.join("src");
    fs::create_dir_all(&src_dir).unwrap();
    let manifest_path = p.join("Cargo.toml");
    let lock_path = p.join("Cargo.lock");
    let lib_src = src_dir.join("lib.rs");
    let bin_out = p.join("target").join("dev").join("mybin");
    fs::create_dir_all(bin_out.parent().unwrap()).unwrap();

    fs::write(
        &manifest_path,
        "[package]\nname = \"pkg\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(&lock_path, "# Cargo.lock\n").unwrap();
    fs::write(&lib_src, "pub fn f() {}\n").unwrap();
    fs::write(&bin_out, "binary content\n").unwrap();

    let outputs = vec![bin_out.clone()];
    let feat = vec!["std".to_string()];

    // Record baseline state
    FastBuildLock::record_state(&RecordStateArgs {
        project_dir: p,
        profile: "dev",
        package: &None,
        features: &feat,
        no_default_features: false,
        all_features: false,
        toolchain_version: "1.80.0",
        action_count: 3,
        root_binary: Some(bin_out.clone()),
        outputs: &outputs,
        stderr: None,
    })
    .unwrap();

    // 1. Fresh hit
    let fresh = FastBuildLock::check_freshness(&FreshnessCheckArgs {
        project_dir: p,
        profile: "dev",
        package: &None,
        features: &feat,
        no_default_features: false,
        all_features: false,
        toolchain_version: "1.80.0",
    });
    assert!(matches!(
        fresh,
        FreshnessResult::Fresh {
            action_count: 3,
            ..
        }
    ));

    // 2. Missing lock for release profile
    let no_lock = FastBuildLock::check_freshness(&FreshnessCheckArgs {
        project_dir: p,
        profile: "release",
        package: &None,
        features: &feat,
        no_default_features: false,
        all_features: false,
        toolchain_version: "1.80.0",
    });
    assert!(matches!(no_lock, FreshnessResult::Stale(r) if r.contains("No build.lock found")));

    // 3. Profile mismatch when lockfile content disagrees with requested profile
    let dev_lock_file = FastBuildLock::lock_path(p, "dev");
    let rel_lock_file = FastBuildLock::lock_path(p, "release");
    fs::copy(&dev_lock_file, &rel_lock_file).unwrap();
    let stale_prof = FastBuildLock::check_freshness(&FreshnessCheckArgs {
        project_dir: p,
        profile: "release",
        package: &None,
        features: &feat,
        no_default_features: false,
        all_features: false,
        toolchain_version: "1.80.0",
    });
    assert!(matches!(stale_prof, FreshnessResult::Stale(r) if r.contains("Profile mismatch")));

    // 3. Target package mismatch
    let stale_pkg = FastBuildLock::check_freshness(&FreshnessCheckArgs {
        project_dir: p,
        profile: "dev",
        package: &Some("other-pkg".to_string()),
        features: &feat,
        no_default_features: false,
        all_features: false,
        toolchain_version: "1.80.0",
    });
    assert!(
        matches!(stale_pkg, FreshnessResult::Stale(r) if r.contains("Target package mismatch"))
    );

    // 4. Feature configuration changed (features, no_default_features, all_features)
    let stale_feat = FastBuildLock::check_freshness(&FreshnessCheckArgs {
        project_dir: p,
        profile: "dev",
        package: &None,
        features: &[],
        no_default_features: false,
        all_features: false,
        toolchain_version: "1.80.0",
    });
    assert!(
        matches!(stale_feat, FreshnessResult::Stale(r) if r.contains("Feature configuration changed"))
    );

    let stale_no_default = FastBuildLock::check_freshness(&FreshnessCheckArgs {
        project_dir: p,
        profile: "dev",
        package: &None,
        features: &feat,
        no_default_features: true,
        all_features: false,
        toolchain_version: "1.80.0",
    });
    assert!(
        matches!(stale_no_default, FreshnessResult::Stale(r) if r.contains("Feature configuration changed"))
    );

    let stale_all_feat = FastBuildLock::check_freshness(&FreshnessCheckArgs {
        project_dir: p,
        profile: "dev",
        package: &None,
        features: &feat,
        no_default_features: false,
        all_features: true,
        toolchain_version: "1.80.0",
    });
    assert!(
        matches!(stale_all_feat, FreshnessResult::Stale(r) if r.contains("Feature configuration changed"))
    );

    // 5. Toolchain version changed
    let stale_tc = FastBuildLock::check_freshness(&FreshnessCheckArgs {
        project_dir: p,
        profile: "dev",
        package: &None,
        features: &feat,
        no_default_features: false,
        all_features: false,
        toolchain_version: "1.81.0",
    });
    assert!(
        matches!(stale_tc, FreshnessResult::Stale(r) if r.contains("Toolchain version changed"))
    );
}

#[test]
fn test_m8_build_lock_file_mutation_and_lock_errors() {
    let tmp = TempDir::new().unwrap();
    let p = tmp.path();

    let src_dir = p.join("src");
    fs::create_dir_all(&src_dir).unwrap();
    let manifest_path = p.join("Cargo.toml");
    let lib_src = src_dir.join("lib.rs");
    let bin_out = p.join("target").join("dev").join("mybin");
    fs::create_dir_all(bin_out.parent().unwrap()).unwrap();

    fs::write(
        &manifest_path,
        "[package]\nname = \"pkg2\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(&lib_src, "pub fn g() {}\n").unwrap();
    fs::write(&bin_out, "out\n").unwrap();

    let outputs = vec![bin_out.clone()];
    FastBuildLock::record_state(&RecordStateArgs {
        project_dir: p,
        profile: "dev",
        package: &None,
        features: &[],
        no_default_features: false,
        all_features: false,
        toolchain_version: "1.80.0",
        action_count: 1,
        root_binary: None,
        outputs: &outputs,
        stderr: None,
    })
    .unwrap();

    // 1. Truncate output artifact (size == 0)
    fs::write(&bin_out, "").unwrap();
    let stale_trunc = FastBuildLock::check_freshness(&FreshnessCheckArgs {
        project_dir: p,
        profile: "dev",
        package: &None,
        features: &[],
        no_default_features: false,
        all_features: false,
        toolchain_version: "1.80.0",
    });
    assert!(matches!(stale_trunc, FreshnessResult::Stale(r) if r.contains("truncated")));

    // 2. Delete output artifact
    fs::remove_file(&bin_out).unwrap();
    let stale_out_del = FastBuildLock::check_freshness(&FreshnessCheckArgs {
        project_dir: p,
        profile: "dev",
        package: &None,
        features: &[],
        no_default_features: false,
        all_features: false,
        toolchain_version: "1.80.0",
    });
    assert!(
        matches!(stale_out_del, FreshnessResult::Stale(r) if r.contains("Output artifact missing"))
    );

    // 3. Delete source file
    fs::remove_file(&lib_src).unwrap();
    let stale_src_del = FastBuildLock::check_freshness(&FreshnessCheckArgs {
        project_dir: p,
        profile: "dev",
        package: &None,
        features: &[],
        no_default_features: false,
        all_features: false,
        toolchain_version: "1.80.0",
    });
    assert!(matches!(stale_src_del, FreshnessResult::Stale(r) if r.contains("Source deleted")));

    // 4. FileLock error paths: opening an existing directory as a lock file fails
    let dir_lock_target = tmp.path().join("a_directory");
    fs::create_dir_all(&dir_lock_target).unwrap();

    let err_excl = FileLock::acquire_exclusive(&dir_lock_target);
    assert!(err_excl.is_err());

    let err_try = FileLock::try_acquire_exclusive(&dir_lock_target);
    assert!(err_try.is_err());

    let err_shared = FileLock::acquire_shared(&dir_lock_target);
    assert!(err_shared.is_err());
}
