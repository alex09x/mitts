/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::core::cas::ContentAddressedStore;
use mitts::core::target_index::{check_or_restore, TargetArtifactIndex};
use std::fs;
use std::sync::{Arc, Mutex};
use tempfile::TempDir;

#[test]
fn test_target_index_save_and_load() {
    let tmp = TempDir::new().unwrap();
    let target_dir = tmp.path().join("target");
    fs::create_dir_all(&target_dir).unwrap();

    let mut index = TargetArtifactIndex::default();
    let dummy_file = target_dir.join("dummy.bin");
    fs::write(&dummy_file, b"binary payload").unwrap();

    index.record(
        "debug/dummy.bin".to_string(),
        "fakehash123".to_string(),
        &dummy_file,
    );

    index.save(&target_dir, "debug").unwrap();

    let loaded = TargetArtifactIndex::load(&target_dir, "debug");
    assert_eq!(loaded.artifacts.len(), 1);
    let record = loaded.artifacts.get("debug/dummy.bin").unwrap();
    assert_eq!(record.cas_hash, "fakehash123");
    assert_eq!(record.size, 14);
}

#[test]
fn test_check_or_restore_skips_intact_and_detects_mutations() {
    let tmp = TempDir::new().unwrap();
    let cas_dir = tmp.path().join("cas");
    let target_dir = tmp.path().join("target");
    fs::create_dir_all(&target_dir).unwrap();

    let cas = ContentAddressedStore::new(cas_dir).unwrap();

    let src_file = tmp.path().join("source_artifact.bin");
    fs::write(&src_file, b"compiled test runner binary v1").unwrap();
    let (hash, size) = cas.store_file(&src_file).unwrap();
    assert_eq!(size, 30);

    let index = Arc::new(Mutex::new(TargetArtifactIndex::default()));
    let dest_path = target_dir.join("debug/deps/test_bin");
    let rel_key = "debug/deps/test_bin";

    // 1. Initial restore: file does not exist on disk
    let restored = check_or_restore(
        Some(&index),
        &cas,
        &hash,
        &dest_path,
        rel_key,
        true,
    )
    .unwrap();
    assert!(restored, "Initial check_or_restore should restore missing file");
    assert!(dest_path.exists());
    assert_eq!(fs::read(&dest_path).unwrap(), b"compiled test runner binary v1");

    // 2. Second call: file is intact on disk -> must skip restore (0 disk writes)
    let restored_again = check_or_restore(
        Some(&index),
        &cas,
        &hash,
        &dest_path,
        rel_key,
        true,
    )
    .unwrap();
    assert!(
        !restored_again,
        "Second check_or_restore on intact file should skip restore"
    );

    // 3. Mutate destination file (simulate corruption or user edit)
    fs::write(&dest_path, b"corrupted payload").unwrap();

    // 4. Third call: corrupted file must be re-restored from CAS
    let restored_after_corrupt = check_or_restore(
        Some(&index),
        &cas,
        &hash,
        &dest_path,
        rel_key,
        true,
    )
    .unwrap();
    assert!(
        restored_after_corrupt,
        "check_or_restore should re-restore mutated/corrupted file"
    );
    assert_eq!(fs::read(&dest_path).unwrap(), b"compiled test runner binary v1");
}

#[cfg(unix)]
#[test]
fn test_hardlink_fast_path_intact_detection() {
    let tmp = TempDir::new().unwrap();
    let cas_dir = tmp.path().join("cas");
    let target_dir = tmp.path().join("target");
    fs::create_dir_all(&target_dir).unwrap();

    let cas = ContentAddressedStore::new(cas_dir).unwrap();

    let src_file = tmp.path().join("shared_tool.bin");
    fs::write(&src_file, b"reusable build tool executable").unwrap();
    let (hash, _) = cas.store_file(&src_file).unwrap();

    let obj_path = cas.object_path(&hash);
    let hardlink_dest = target_dir.join("debug/shared_tool");
    fs::create_dir_all(hardlink_dest.parent().unwrap()).unwrap();
    fs::hard_link(&obj_path, &hardlink_dest).unwrap();

    // Empty index (no recorded sidecar)
    let index = TargetArtifactIndex::default();

    // Should return true via shared inode fast path even without sidecar record
    assert!(
        index.is_intact(&cas, &hash, &hardlink_dest, "debug/shared_tool"),
        "Hardlinked file sharing CAS inode must be recognized as intact"
    );

    // If destination is deleted, is_intact must return false
    fs::remove_file(&hardlink_dest).unwrap();
    assert!(!index.is_intact(&cas, &hash, &hardlink_dest, "debug/shared_tool"));
}
