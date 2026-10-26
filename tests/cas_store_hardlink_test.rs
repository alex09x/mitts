/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! The experimental store-hardlink restore shares the read-only object's inode with the
//! workspace copy and never gives it write bits; the default keeps cloning or copying.

#![cfg(unix)]

use mitts::core::cas::ContentAddressedStore;
use mitts::core::reflink::StoreHardlinks;
use mitts::core::target_index::{check_or_restore, TargetArtifactIndex};
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::os::unix::fs::PermissionsExt;
use std::sync::{Arc, Mutex};
use tempfile::TempDir;

#[test]
fn a_store_hardlink_restore_shares_the_read_only_inode() {
    let temp = TempDir::new().unwrap();
    let cas = ContentAddressedStore::new(temp.path().join("cas"))
        .unwrap()
        .with_store_hardlinks(StoreHardlinks::Always);
    assert_eq!(cas.store_hardlinks(), StoreHardlinks::Always);
    let (lib_hash, _) = cas.store_bytes(b"rlib bytes").unwrap();
    let (bin_hash, _) = cas.store_bytes(b"test binary bytes").unwrap();

    let lib = temp.path().join("target/deps/libx.rlib");
    cas.restore_object(&lib_hash, &lib, false).unwrap();
    let (obj, dest) = (
        fs::metadata(cas.object_path(&lib_hash)).unwrap(),
        fs::metadata(&lib).unwrap(),
    );
    assert_eq!(
        (obj.dev(), obj.ino()),
        (dest.dev(), dest.ino()),
        "same inode"
    );
    assert_eq!(dest.permissions().mode() & 0o777, 0o444);

    let bin = temp.path().join("target/deps/test_bin");
    cas.restore_object(&bin_hash, &bin, true).unwrap();
    let dest = fs::metadata(&bin).unwrap();
    assert_eq!(
        dest.permissions().mode() & 0o777,
        0o555,
        "executable, never writable"
    );
    assert_eq!(fs::read(&bin).unwrap(), b"test binary bytes");

    // The target index recognises the link as intact: a second check restores nothing.
    let index = Arc::new(Mutex::new(TargetArtifactIndex::default()));
    assert!(
        !check_or_restore(Some(&index), &cas, &lib_hash, &lib, "deps/libx.rlib", false).unwrap()
    );

    // A link that gained write bits is no longer trusted and is replaced.
    fs::set_permissions(&lib, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(
        check_or_restore(Some(&index), &cas, &lib_hash, &lib, "deps/libx.rlib", false).unwrap()
    );
    assert_eq!(
        fs::metadata(&lib).unwrap().permissions().mode() & 0o777,
        0o444
    );
}

#[test]
fn the_default_never_links_and_auto_links_only_without_reflink() {
    let temp = TempDir::new().unwrap();
    let cas = ContentAddressedStore::new(temp.path().join("cas")).unwrap();
    assert_eq!(
        cas.store_hardlinks(),
        StoreHardlinks::Never,
        "the documented default never links store objects into the workspace"
    );
    let fallback = ContentAddressedStore::new(temp.path().join("cas3"))
        .unwrap()
        .with_store_hardlinks(StoreHardlinks::Fallback);
    let (fb_hash, _) = fallback.store_bytes(b"rlib bytes").unwrap();
    let fb_dest = temp.path().join("target/deps/libfb.rlib");
    fallback.restore_object(&fb_hash, &fb_dest, false).unwrap();
    let (fb_obj, fb_out) = (
        fs::metadata(fallback.object_path(&fb_hash)).unwrap(),
        fs::metadata(&fb_dest).unwrap(),
    );
    assert_eq!(fs::read(&fb_dest).unwrap(), b"rlib bytes");
    if fb_obj.ino() == fb_out.ino() {
        assert_eq!(
            fb_out.permissions().mode() & 0o777,
            0o444,
            "a link is read-only"
        );
    } else {
        assert_eq!(
            fb_out.permissions().mode() & 0o777,
            0o644,
            "a clone is a private copy"
        );
    }
    let (hash, _) = cas.store_bytes(b"rlib bytes").unwrap();
    let dest = temp.path().join("target/deps/libx.rlib");
    cas.restore_object(&hash, &dest, false).unwrap();
    let (obj, out) = (
        fs::metadata(cas.object_path(&hash)).unwrap(),
        fs::metadata(&dest).unwrap(),
    );
    assert_ne!(obj.ino(), out.ino(), "a copy or clone, not a link");
    assert_eq!(out.permissions().mode() & 0o777, 0o644);
}
