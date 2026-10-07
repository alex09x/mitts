/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::core::hash::Sha256;
use mitts::core::lock::FileLock;
use std::io::Write;
use tempfile::TempDir;

#[test]
fn test_m8_filelock_shared_and_try_exclusive() {
    let tmp = TempDir::new().unwrap();
    let lock_path = tmp.path().join("test.lock");

    // Two shared locks can coexist
    let lock1 = FileLock::acquire_shared(&lock_path).unwrap();
    assert_eq!(lock1.path(), lock_path);
    let lock2 = FileLock::acquire_shared(&lock_path).unwrap();
    assert_eq!(lock2.path(), lock_path);

    // Exclusive try-acquire should return None because shared locks are held
    let excl_try = FileLock::try_acquire_exclusive(&lock_path).unwrap();
    assert!(excl_try.is_none());

    drop(lock1);
    drop(lock2);

    // Now exclusive try-acquire should succeed
    let excl_success = FileLock::try_acquire_exclusive(&lock_path).unwrap();
    assert!(excl_success.is_some());

    // Second exclusive try-acquire returns None
    let excl_second = FileLock::try_acquire_exclusive(&lock_path).unwrap();
    assert!(excl_second.is_none());
}

#[test]
fn test_m8_sha256_write_trait_and_padding_branches() {
    // 1. Default trait implementation
    let mut hasher: Sha256 = Default::default();
    write!(hasher, "streamed input ").unwrap();
    hasher.flush().unwrap();
    hasher.update(b"and update");
    let digest = mitts::core::hash::bytes_to_hex(&hasher.finalize());
    assert_eq!(digest, Sha256::digest(b"streamed input and update"));

    // 2. Padding with buffer_len > 56 (forces two blocks in finalize)
    // 58 bytes input
    let data_58 = [b'A'; 58];
    let hash_58 = Sha256::digest(&data_58);
    assert_eq!(hash_58.len(), 64);

    // 60 bytes input
    let data_60 = [b'B'; 60];
    let hash_60 = Sha256::digest(&data_60);
    assert_eq!(hash_60.len(), 64);

    // 3. digest_file on nonexistent path
    let tmp = TempDir::new().unwrap();
    let missing = tmp.path().join("missing.file");
    assert!(Sha256::digest_file(&missing).is_err());

    // 4. Large stream > 64KB (chunked reader loop)
    let large_data = vec![0x55u8; 131072]; // 128 KB
    let cursor = std::io::Cursor::new(&large_data);
    let large_hash = Sha256::digest_reader(cursor).unwrap();
    assert_eq!(large_hash, Sha256::digest(&large_data));
}
