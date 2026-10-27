/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A named artifact (`<store>/artifacts/lib<crate>-<hash>.rlib`) is a hardlink to the store
//! object, the path `--extern` hands to rustc in native mode without any restore step. An
//! object that gained write bits through such a link and was overwritten must be caught
//! there as well, not only on the restore path.

#![cfg(unix)]

use mitts::core::cas::ContentAddressedStore;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use tempfile::TempDir;

#[test]
fn a_named_artifact_whose_object_was_overwritten_through_a_link_is_refused() {
    let temp = TempDir::new().unwrap();
    let cas = ContentAddressedStore::new(temp.path().join("cas")).unwrap();
    let (hash, _) = cas.store_bytes(b"rlib bytes v1").unwrap();
    let alias = cas
        .named_artifact_path_with_disambiguator("mycrate", &hash, "0123456789abcdef", "rlib")
        .unwrap();
    assert_eq!(
        fs::metadata(&alias).unwrap().ino(),
        fs::metadata(cas.object_path(&hash)).unwrap().ino(),
        "the alias shares the object's inode"
    );
    // Asking again is a read-only hit.
    assert_eq!(
        cas.named_artifact_path_with_disambiguator("mycrate", &hash, "0123456789abcdef", "rlib")
            .unwrap(),
        alias
    );

    // Someone made the shared inode writable through the alias and overwrote it.
    fs::set_permissions(&alias, fs::Permissions::from_mode(0o644)).unwrap();
    fs::write(&alias, b"rlib bytes v2").unwrap();
    let err = cas
        .named_artifact_path_with_disambiguator("mycrate", &hash, "0123456789abcdef", "rlib")
        .expect_err("a modified store object is refused, not handed to rustc");
    assert!(err.to_string().contains("corrupted"), "{err}");
    assert!(
        !cas.object_path(&hash).exists(),
        "the corrupted object is removed from the store"
    );

    // Write bits without a modification are harmless: the object is re-hashed and resealed.
    let (hash2, _) = cas.store_bytes(b"rlib bytes v3").unwrap();
    let alias2 = cas
        .named_artifact_path_with_disambiguator("other", &hash2, "fedcba9876543210", "rlib")
        .unwrap();
    fs::set_permissions(&alias2, fs::Permissions::from_mode(0o644)).unwrap();
    let again = cas
        .named_artifact_path_with_disambiguator("other", &hash2, "fedcba9876543210", "rlib")
        .unwrap();
    assert_eq!(again, alias2);
    assert_eq!(
        fs::metadata(cas.object_path(&hash2))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o444,
        "resealed"
    );
}
