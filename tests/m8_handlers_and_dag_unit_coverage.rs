/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::cli::handlers::{handle_clean, handle_explain, handle_gc};
use mitts::cli::{CleanArgs, ExplainArgs, GcArgs};
use mitts::core::build_lock::FastBuildLock;
use std::fs;
use tempfile::TempDir;

#[test]
fn test_m8_handlers_explain_clean_and_gc_comprehensive() {
    let isolated_cache = tempfile::tempdir().expect("isolated cache dir");
    let temp_dir = TempDir::new().expect("tempdir");
    let proj_dir = temp_dir.path().join("proj");
    fs::create_dir_all(&proj_dir).unwrap();

    let manifest = proj_dir.join("Cargo.toml");
    let src_dir = proj_dir.join("src");
    fs::create_dir_all(&src_dir).unwrap();

    fs::write(
        &manifest,
        "[package]\nname = \"hand-pkg\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(src_dir.join("main.rs"), "fn main() {}\n").unwrap();

    // 1. handle_explain with valid manifest
    let exp_args = ExplainArgs {
        cache_dir: Some(isolated_cache.path().to_path_buf()),
        manifest_path: Some(manifest.clone()),
        release: true,
        ..Default::default()
    };
    assert_eq!(handle_explain(&exp_args), 0);

    // 2. handle_explain with invalid manifest -> 1
    let err_exp = ExplainArgs {
        cache_dir: Some(isolated_cache.path().to_path_buf()),
        manifest_path: Some(temp_dir.path().join("Nonexistent.toml")),
        ..Default::default()
    };
    assert_eq!(handle_explain(&err_exp), 1);

    // 3. handle_clean error path
    let err_clean = CleanArgs {
        cache_dir: Some(isolated_cache.path().to_path_buf()),
        manifest_path: Some(temp_dir.path().join("Nonexistent.toml")),
        ..Default::default()
    };
    assert_eq!(handle_clean(&err_clean), 1);

    // 4. Create locks, target dirs, build.log for both release and dev
    let mitts_dir = proj_dir.join("target").join("mitts");
    let target_rel = mitts_dir.join("release");
    fs::create_dir_all(&target_rel).unwrap();
    fs::write(target_rel.join("artifact.bin"), b"binary").unwrap();
    fs::write(mitts_dir.join("build.log"), "sample log").unwrap();

    let rel_lock = FastBuildLock::lock_path(&proj_dir, "release");
    let dev_lock = FastBuildLock::lock_path(&proj_dir, "dev");
    let check_lock = FastBuildLock::lock_path(&proj_dir, "check");
    fs::write(&rel_lock, "{}").unwrap();
    fs::write(&dev_lock, "{}").unwrap();
    fs::write(&check_lock, "{}").unwrap();

    // Clean release mode
    let clean_rel_args = CleanArgs {
        cache_dir: Some(isolated_cache.path().to_path_buf()),
        manifest_path: Some(manifest.clone()),
        release: true,
        ..Default::default()
    };
    assert_eq!(handle_clean(&clean_rel_args), 0);
    assert!(!rel_lock.exists());

    // Clean dev mode + cache prune
    let cache_dir = temp_dir.path().join("cache");
    let clean_dev_args = CleanArgs {
        manifest_path: Some(manifest),
        release: false,
        cache: true,
        cache_dir: Some(cache_dir.clone()),
        ..Default::default()
    };
    assert_eq!(handle_clean(&clean_dev_args), 0);
    assert!(!dev_lock.exists());
    assert!(!check_lock.exists());

    // 5. handle_gc with max_bytes and max_age_secs
    let gc_args = GcArgs {
        cache_dir: Some(cache_dir),
        max_bytes: Some(1024 * 1024),
        max_age_secs: Some(3600),
    };
    assert_eq!(handle_gc(&gc_args), 0);
}
