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
use mitts::core::lock::FileLock;
use std::fs;

#[test]
fn test_m8_file_lock_exclusive_and_try_lock() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let lock_file = temp_dir.path().join("sub").join("test.lock");

    // 1. acquire_exclusive creates parent dir and locks
    let lock1 = FileLock::acquire_exclusive(&lock_file).expect("acquire exclusive");
    assert_eq!(lock1.path(), lock_file);

    // 2. try_acquire_exclusive on locked file should return None
    let try_lock = FileLock::try_acquire_exclusive(&lock_file).expect("try acquire");
    assert!(try_lock.is_none(), "Expected lock collision to return None");

    // 3. Drop lock1, now try_acquire_exclusive should succeed
    drop(lock1);
    let lock2 = FileLock::try_acquire_exclusive(&lock_file)
        .expect("try acquire after drop")
        .expect("should acquire lock");
    assert_eq!(lock2.path(), lock_file);
}

#[test]
fn test_m8_file_lock_shared_concurrency() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let lock_file = temp_dir.path().join("shared.lock");

    // Shared locks can be acquired concurrently
    let sh1 = FileLock::acquire_shared(&lock_file).expect("acquire shared 1");
    let sh2 = FileLock::acquire_shared(&lock_file).expect("acquire shared 2");
    assert_eq!(sh1.path(), lock_file);
    assert_eq!(sh2.path(), lock_file);

    // Exclusive try-lock while shared is held should return None
    let ex_try = FileLock::try_acquire_exclusive(&lock_file).expect("try exclusive");
    assert!(ex_try.is_none());

    drop(sh1);
    drop(sh2);
}

#[test]
fn test_m8_cas_store_and_restore_bytes() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let cas = ContentAddressedStore::new(temp_dir.path().join("cas")).expect("init cas");

    let payload = b"Hello, CAS immutable storage!";
    let (hash, size) = cas.store_bytes(payload).expect("store bytes");
    assert_eq!(size, payload.len() as u64);
    assert!(cas.has_object(&hash));
    assert!(cas.verify_object(&hash).expect("verify object"));

    // Storing identical bytes again should hit the fast-path
    let (hash2, size2) = cas.store_bytes(payload).expect("store bytes dup");
    assert_eq!(hash, hash2);
    assert_eq!(size, size2);

    // Restore to destination
    let dest_file = temp_dir.path().join("restored.bin");
    cas.restore_object(&hash, &dest_file, true)
        .expect("restore object");
    assert_eq!(fs::read(&dest_file).unwrap(), payload);

    // Restore non-existent object
    let missing_res = cas.restore_object(
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        &dest_file,
        false,
    );
    assert!(missing_res.is_err());
}

#[test]
fn test_m8_cas_corrupted_object_recovery() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let cas = ContentAddressedStore::new(temp_dir.path().join("cas")).expect("init cas");

    let src_file = temp_dir.path().join("source.txt");
    fs::write(&src_file, "original content to store").unwrap();

    let (hash, _) = cas.store_file(&src_file).expect("store file");
    assert!(cas.verify_object(&hash).unwrap());

    // Corrupt the file in CAS
    let obj_path = cas.object_path(&hash);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&obj_path, fs::Permissions::from_mode(0o644));
    }
    fs::write(&obj_path, "corrupted tampered data").unwrap();

    // Verification should now return false
    assert!(!cas.verify_object(&hash).unwrap());

    // Attempting restore on corrupted object should delete it and return error
    let dest = temp_dir.path().join("out.txt");
    let res = cas.restore_object(&hash, &dest, false);
    assert!(res.is_err());
    assert!(!cas.has_object(&hash), "Corrupted object should be removed");

    // Storing source again should replace corrupted entry
    let (hash_re, _) = cas.store_file(&src_file).expect("re-store file");
    assert_eq!(hash, hash_re);
    assert!(cas.has_object(&hash_re));
    assert!(cas.verify_object(&hash_re).unwrap());
}
