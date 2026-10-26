/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A restore trusts the immutable store by default and does not read the object's
//! bytes; `with_restore_verification(true)` (or `MITTS_VERIFY_RESTORE=1`) re-hashes
//! first and rejects a corrupted object.

use mitts::core::cas::ContentAddressedStore;
use std::fs;
use tempfile::TempDir;

fn tamper(cas: &ContentAddressedStore, hash: &str, bytes: &[u8]) {
    let obj = cas.object_path(hash);
    let mut perms = fs::metadata(&obj).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    perms.set_readonly(false);
    fs::set_permissions(&obj, perms).unwrap();
    fs::write(&obj, bytes).unwrap();
}

#[test]
fn a_default_restore_links_the_object_without_hashing_it() {
    let temp = TempDir::new().unwrap();
    let cas = ContentAddressedStore::new(temp.path().join("cas")).unwrap();
    assert!(!cas.verifies_on_restore());
    let (hash, _) = cas.store_bytes(b"artifact bytes v1").unwrap();

    // Same length, different content: only a full read could tell.
    tamper(&cas, &hash, b"artifact bytes v2");
    let dest = temp.path().join("restored");
    cas.restore_object(&hash, &dest, false).unwrap();
    assert_eq!(fs::read(&dest).unwrap(), b"artifact bytes v2");
    assert!(
        cas.has_object(&hash),
        "a trusting restore never deletes objects"
    );
    assert!(
        !cas.verify_object(&hash).unwrap(),
        "verification is still available on demand"
    );
}

#[test]
fn a_verifying_restore_rejects_and_removes_a_corrupted_object() {
    let temp = TempDir::new().unwrap();
    let cas = ContentAddressedStore::new(temp.path().join("cas"))
        .unwrap()
        .with_restore_verification(true);
    assert!(cas.verifies_on_restore());
    let (hash, _) = cas.store_bytes(b"artifact bytes v1").unwrap();
    tamper(&cas, &hash, b"artifact bytes v2");
    let dest = temp.path().join("restored");
    assert!(cas.restore_object(&hash, &dest, false).is_err());
    assert!(!cas.has_object(&hash));
    assert!(!dest.exists());
}
