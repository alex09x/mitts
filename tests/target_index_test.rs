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

    // This test mutates the restored file in place to exercise the sidecar checks, so
    // it restores private copies rather than read-only links to the store.
    let cas = ContentAddressedStore::new(cas_dir)
        .unwrap()
        .with_store_hardlinks(mitts::core::reflink::StoreHardlinks::Never);

    let src_file = tmp.path().join("source_artifact.bin");
    fs::write(&src_file, b"compiled test runner binary v1").unwrap();
    let (hash, size) = cas.store_file(&src_file).unwrap();
    assert_eq!(size, 30);

    let index = Arc::new(Mutex::new(TargetArtifactIndex::default()));
    let dest_path = target_dir.join("debug/deps/test_bin");
    let rel_key = "debug/deps/test_bin";

    // 1. Initial restore: file does not exist on disk
    let restored = check_or_restore(Some(&index), &cas, &hash, &dest_path, rel_key, true).unwrap();
    assert!(
        restored,
        "Initial check_or_restore should restore missing file"
    );
    assert!(dest_path.exists());
    assert_eq!(
        fs::read(&dest_path).unwrap(),
        b"compiled test runner binary v1"
    );

    // 2. Second call: file is intact on disk -> must skip restore (0 disk writes)
    let restored_again =
        check_or_restore(Some(&index), &cas, &hash, &dest_path, rel_key, true).unwrap();
    assert!(
        !restored_again,
        "Second check_or_restore on intact file should skip restore"
    );

    // 3a. Mutate destination file with different size
    fs::write(&dest_path, b"corrupted payload").unwrap();
    let restored_after_size_corrupt =
        check_or_restore(Some(&index), &cas, &hash, &dest_path, rel_key, true).unwrap();
    assert!(
        restored_after_size_corrupt,
        "check_or_restore should re-restore size-mutated file"
    );
    assert_eq!(
        fs::read(&dest_path).unwrap(),
        b"compiled test runner binary v1"
    );

    // 3b. Mutate destination file with SAME size but different content (mtime change)
    std::thread::sleep(std::time::Duration::from_millis(15));
    fs::write(&dest_path, b"compiled test runner binary v2").unwrap();
    assert_eq!(fs::metadata(&dest_path).unwrap().len(), 30);
    let restored_after_same_size =
        check_or_restore(Some(&index), &cas, &hash, &dest_path, rel_key, true).unwrap();
    assert!(
        restored_after_same_size,
        "check_or_restore should re-restore same-size content mutation"
    );
    assert_eq!(
        fs::read(&dest_path).unwrap(),
        b"compiled test runner binary v1"
    );

    // 3c. Truncate destination file to 0 bytes
    fs::write(&dest_path, b"").unwrap();
    let restored_after_trunc =
        check_or_restore(Some(&index), &cas, &hash, &dest_path, rel_key, true).unwrap();
    assert!(
        restored_after_trunc,
        "check_or_restore should re-restore truncated empty file"
    );
    assert_eq!(
        fs::read(&dest_path).unwrap(),
        b"compiled test runner binary v1"
    );
}

#[cfg(unix)]
#[test]
fn test_hardlink_fast_path_intact_detection() {
    use std::os::unix::fs::PermissionsExt;

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
    fs::set_permissions(&hardlink_dest, fs::Permissions::from_mode(0o555)).unwrap();

    // Empty index (no recorded sidecar)
    let index = TargetArtifactIndex::default();

    // Should return true via shared inode fast path when read-only
    assert!(
        index.is_intact(&cas, &hash, &hardlink_dest, "debug/shared_tool"),
        "Hardlinked file sharing CAS inode must be recognized as intact when read-only"
    );

    // If file is made writable (chmod u+w), fast path must reject it to prevent silent corruption
    fs::set_permissions(&hardlink_dest, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(
        !index.is_intact(&cas, &hash, &hardlink_dest, "debug/shared_tool"),
        "Writable hardlink (chmod u+w) must be rejected by intact check"
    );

    // Reset to read-only
    fs::set_permissions(&hardlink_dest, fs::Permissions::from_mode(0o555)).unwrap();

    // If destination is deleted, is_intact must return false
    fs::remove_file(&hardlink_dest).unwrap();
    assert!(!index.is_intact(&cas, &hash, &hardlink_dest, "debug/shared_tool"));
}

#[test]
fn test_target_index_dirty_flag_skips_unnecessary_writes() {
    let tmp = TempDir::new().unwrap();
    let target_dir = tmp.path().join("target");
    fs::create_dir_all(&target_dir).unwrap();

    let mut index = TargetArtifactIndex::default();
    assert!(!index.dirty);

    // Save on clean index should not create file
    index.save(&target_dir, "debug").unwrap();
    assert!(!TargetArtifactIndex::index_path(&target_dir, "debug").exists());

    // Record dummy file
    let dummy = target_dir.join("dummy.bin");
    fs::write(&dummy, b"payload").unwrap();
    index.record("debug/dummy.bin".to_string(), "hash1".to_string(), &dummy);
    assert!(index.dirty);

    // Save persists and clears dirty
    index.save(&target_dir, "debug").unwrap();
    assert!(!index.dirty);
    assert!(TargetArtifactIndex::index_path(&target_dir, "debug").exists());

    // Record identical metadata does NOT set dirty
    index.record("debug/dummy.bin".to_string(), "hash1".to_string(), &dummy);
    assert!(!index.dirty);
}
