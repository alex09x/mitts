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
use mitts::core::reflink::link_intra_workspace;
use std::fs;

#[test]
fn test_intra_workspace_hardlink_basic_and_inode_match() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    let src = root.join("src_file.rlib");
    fs::write(&src, b"rust library bytecode v1").unwrap();

    let dst = root.join("deps").join("src_file.rlib");
    link_intra_workspace(&src, &dst, false).unwrap();

    assert!(dst.is_file());
    assert_eq!(fs::read(&dst).unwrap(), b"rust library bytecode v1");

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let src_meta = fs::metadata(&src).unwrap();
        let dst_meta = fs::metadata(&dst).unwrap();
        assert_eq!(src_meta.ino(), dst_meta.ino(), "Hardlinked files must share the same inode");
        assert_eq!(src_meta.nlink(), 2, "nlink must be 2 after hardlink");
    }
}

#[test]
fn test_intra_workspace_hardlink_safety_when_nlink_gt_1() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    let src = root.join("source.bin");
    fs::write(&src, b"binary content").unwrap();

    // Create an initial hardlink so src has nlink == 2
    let alias1 = root.join("alias1.bin");
    link_intra_workspace(&src, &alias1, true).unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert_eq!(fs::metadata(&src).unwrap().nlink(), 2);
    }

    // Attempting another link from src (now nlink == 2) must fallback safely to reflink/copy
    // rather than creating a 3rd hardlink, preserving strict workspace isolation.
    let alias2 = root.join("alias2.bin");
    link_intra_workspace(&src, &alias2, true).unwrap();
    assert!(alias2.is_file());
    assert_eq!(fs::read(&alias2).unwrap(), b"binary content");

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let src_meta = fs::metadata(&src).unwrap();
        let alias2_meta = fs::metadata(&alias2).unwrap();
        assert_eq!(src_meta.nlink(), 2, "src nlink must remain 2");
        assert_eq!(alias2_meta.nlink(), 1, "alias2 must have nlink 1 (reflink/copy fallback)");
        assert_ne!(src_meta.ino(), alias2_meta.ino(), "alias2 must have an independent inode");
    }
}

#[test]
fn test_intra_workspace_rebuild_cas_isolation() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    let cas_dir = root.join("cas");
    let cas = ContentAddressedStore::new(cas_dir).unwrap();
    let target_debug = root.join("target").join("debug");
    fs::create_dir_all(&target_debug).unwrap();

    // 1. Initial build: compile binary v1
    let bin_staging = root.join("staging").join("my_app");
    fs::create_dir_all(bin_staging.parent().unwrap()).unwrap();
    fs::write(&bin_staging, b"compiled binary v1 content").unwrap();

    let dest = target_debug.join("my-app");
    let alt_path = target_debug.join("my_app");

    // Mitts stages binary from staging to dest
    let _ = fs::remove_file(&dest);
    mitts::core::reflink_or_copy(&bin_staging, &dest, true).unwrap();

    // Store in CAS
    let (hash_v1, _) = cas.store_file(&dest).unwrap();
    let obj_path_v1 = cas.object_path(&hash_v1);
    assert!(obj_path_v1.is_file());
    assert_eq!(fs::read(&obj_path_v1).unwrap(), b"compiled binary v1 content");

    // Link alt_path
    link_intra_workspace(&dest, &alt_path, true).unwrap();
    assert_eq!(fs::read(&dest).unwrap(), b"compiled binary v1 content");
    assert_eq!(fs::read(&alt_path).unwrap(), b"compiled binary v1 content");

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert_eq!(fs::metadata(&dest).unwrap().ino(), fs::metadata(&alt_path).unwrap().ino());
    }

    // 2. Modified source build: compile binary v2
    let bin_staging_v2 = root.join("staging_v2").join("my_app");
    fs::create_dir_all(bin_staging_v2.parent().unwrap()).unwrap();
    fs::write(&bin_staging_v2, b"compiled binary v2 updated content").unwrap();

    // Mitts unlinks dest before writing compiler output
    let _ = fs::remove_file(&dest);
    mitts::core::reflink_or_copy(&bin_staging_v2, &dest, true).unwrap();

    // Store new object in CAS
    let (hash_v2, _) = cas.store_file(&dest).unwrap();
    assert_ne!(hash_v1, hash_v2);

    // Link alt_path to new dest
    link_intra_workspace(&dest, &alt_path, true).unwrap();

    // 3. Verifications
    assert_eq!(fs::read(&dest).unwrap(), b"compiled binary v2 updated content");
    assert_eq!(fs::read(&alt_path).unwrap(), b"compiled binary v2 updated content");

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert_eq!(fs::metadata(&dest).unwrap().ino(), fs::metadata(&alt_path).unwrap().ino());
    }

    // Crucial check: verify CAS object v1 was NOT mutated or truncated!
    assert_eq!(
        fs::read(&obj_path_v1).unwrap(),
        b"compiled binary v1 content",
        "CAS object for previous build must remain completely intact and unmutated"
    );
    let obj_path_v2 = cas.object_path(&hash_v2);
    assert_eq!(
        fs::read(&obj_path_v2).unwrap(),
        b"compiled binary v2 updated content"
    );
}
