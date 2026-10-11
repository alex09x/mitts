/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::rust::builder::build;
use mitts::adapter::rust::types::RustBuildOptions;
use mitts::core::cas::ContentAddressedStore;
use mitts::core::reflink::link_intra_workspace;
use mitts::core::Sha256;
use std::fs;

#[test]
fn test_intra_workspace_hardlink_basic_and_inode_match() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    let src = root.join("src_file.rlib");
    fs::write(&src, b"rust library bytecode v1").unwrap();

    let dst = root.join("deps").join("src_file.rlib");
    link_intra_workspace(&src, &dst, &[root], false).unwrap();

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
fn test_intra_workspace_hardlink_chaining_and_cas_isolation() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    let ws_dir = root.join("workspace").join("target");
    fs::create_dir_all(&ws_dir).unwrap();

    let src = ws_dir.join("source.bin");
    fs::write(&src, b"binary content").unwrap();

    // 1. Create an initial hardlink so src has nlink == 2
    let alias1 = ws_dir.join("alias1.bin");
    link_intra_workspace(&src, &alias1, &[&ws_dir], true).unwrap();

    // 2. Chaining: link_intra_workspace(&alias1, &alias2) must succeed with zero-copy hardlink!
    let alias2 = ws_dir.join("alias2.bin");
    link_intra_workspace(&alias1, &alias2, &[&ws_dir], true).unwrap();
    assert!(alias2.is_file());
    assert_eq!(fs::read(&alias2).unwrap(), b"binary content");

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let src_meta = fs::metadata(&src).unwrap();
        let alias1_meta = fs::metadata(&alias1).unwrap();
        let alias2_meta = fs::metadata(&alias2).unwrap();
        assert_eq!(src_meta.ino(), alias1_meta.ino());
        assert_eq!(src_meta.ino(), alias2_meta.ino(), "Chained aliases must share the same inode");
        assert_eq!(src_meta.nlink(), 3, "Chained hardlinks must yield nlink == 3");
    }

    // 3. Fast-path: repeated link to already identical inode must be a no-op
    link_intra_workspace(&src, &alias1, &[&ws_dir], true).unwrap();

    // 4. CAS isolation: an object in CAS objects directory must NEVER be hardlinked into workspace
    let cas_dir = root.join("cas");
    let cas = ContentAddressedStore::new(cas_dir).unwrap();
    let (cas_hash, _) = cas.store_file(&src).unwrap();
    let cas_obj = cas.object_path(&cas_hash);
    assert!(cas_obj.is_file());

    let ws_dst = ws_dir.join("from_cas.bin");
    link_intra_workspace(&cas_obj, &ws_dst, &[&ws_dir], true).unwrap();
    assert!(ws_dst.is_file());

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let cas_meta = fs::metadata(&cas_obj).unwrap();
        let ws_meta = fs::metadata(&ws_dst).unwrap();
        assert_ne!(
            cas_meta.ino(),
            ws_meta.ino(),
            "CAS object must never be hardlinked directly into workspace"
        );
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
    link_intra_workspace(&dest, &alt_path, &[&target_debug], true).unwrap();
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
    link_intra_workspace(&dest, &alt_path, &[&target_debug], true).unwrap();

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

#[test]
fn test_intra_workspace_e2e_rebuild_cas_isolation_and_inode_match() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    let cache_dir = root.join("cache");
    let pkg_dir = root.join("my-hyphen-pkg");
    let src_dir = pkg_dir.join("src");
    fs::create_dir_all(&src_dir).unwrap();

    fs::write(
        pkg_dir.join("Cargo.toml"),
        r#"
[package]
name = "my-hyphen-pkg"
version = "0.1.0"
edition = "2021"
"#,
    )
    .unwrap();

    fs::write(
        pkg_dir.join("Cargo.lock"),
        r#"
version = 3

[[package]]
name = "my-hyphen-pkg"
version = "0.1.0"
"#,
    )
    .unwrap();

    let main_rs = src_dir.join("main.rs");
    fs::write(&main_rs, "fn main() { println!(\"binary_output_v1\"); }\n").unwrap();

    let target_dir = pkg_dir.join("target").join("mitts");
    let opt = RustBuildOptions {
        manifest_path: Some(pkg_dir.join("Cargo.toml")),
        target_dir: Some(target_dir.clone()),
        cache_dir: Some(cache_dir.clone()),
        native: true,
        ..Default::default()
    };

    // 1. First compilation cycle (v1)
    let (_plan1, res1) = build(&opt).expect("Build v1 must succeed");
    assert!(
        res1.success,
        "Build v1 failed: exit_code={}, stderr={}",
        res1.exit_code, res1.stderr
    );

    let debug_dir = target_dir.join("debug");
    let dest = debug_dir.join("my-hyphen-pkg");
    let alt_path = debug_dir.join("my_hyphen_pkg");

    assert!(dest.is_file(), "Primary binary must exist: {:?}", dest);
    assert!(
        alt_path.is_file(),
        "Alternate binary alias must exist: {:?}",
        alt_path
    );

    // Verify executions output v1
    let out_dest_v1 = std::process::Command::new(&dest).output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out_dest_v1.stdout).trim(),
        "binary_output_v1"
    );
    let out_alt_v1 = std::process::Command::new(&alt_path).output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out_alt_v1.stdout).trim(),
        "binary_output_v1"
    );

    // Locate CAS object for v1
    let hash_v1 = Sha256::digest_file(&dest).unwrap();
    let cas = ContentAddressedStore::new(cache_dir.join("cas")).unwrap();
    let obj_path_v1 = cas.object_path(&hash_v1);
    assert!(
        obj_path_v1.is_file(),
        "CAS object v1 must exist: {:?}",
        obj_path_v1
    );
    let cas_obj_v1_sha = Sha256::digest_file(&obj_path_v1).unwrap();
    assert_eq!(cas_obj_v1_sha, hash_v1);

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let dest_meta = fs::metadata(&dest).unwrap();
        let alt_meta = fs::metadata(&alt_path).unwrap();
        let cas_meta = fs::metadata(&obj_path_v1).unwrap();

        // Invariant 1: dest and alt_path share the same inode via intra-workspace hardlink
        assert_eq!(
            dest_meta.ino(),
            alt_meta.ino(),
            "dest and alt_path within workspace must share the same inode via intra-workspace hardlink"
        );

        // Invariant 2: dest must NOT share inode with CAS object (strict invariant)
        assert_ne!(
            dest_meta.ino(),
            cas_meta.ino(),
            "dest in workspace must NOT share inode with CAS object (strict invariant)"
        );
    }

    // 2. Second compilation cycle with modified source code (v2)
    fs::write(
        &main_rs,
        "fn main() { println!(\"binary_output_v2_modified\"); }\n",
    )
    .unwrap();

    let (_plan2, res2) = build(&opt).expect("Build v2 must succeed");
    assert!(
        res2.success,
        "Build v2 failed: exit_code={}, stderr={}",
        res2.exit_code, res2.stderr
    );

    // Verify executions output v2
    let out_dest_v2 = std::process::Command::new(&dest).output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out_dest_v2.stdout).trim(),
        "binary_output_v2_modified"
    );
    let out_alt_v2 = std::process::Command::new(&alt_path).output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out_alt_v2.stdout).trim(),
        "binary_output_v2_modified"
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let dest_meta_v2 = fs::metadata(&dest).unwrap();
        let alt_meta_v2 = fs::metadata(&alt_path).unwrap();
        assert_eq!(
            dest_meta_v2.ino(),
            alt_meta_v2.ino(),
            "dest and alt_path after rebuild must share the same inode"
        );
    }

    // Invariant 3: sha256(cas_object_v1) is strictly unchanged after v2 rebuild!
    assert!(
        obj_path_v1.is_file(),
        "CAS object v1 must still exist after v2 rebuild"
    );
    let cas_obj_v1_sha_after = Sha256::digest_file(&obj_path_v1).unwrap();
    assert_eq!(
        cas_obj_v1_sha_after, cas_obj_v1_sha,
        "CAS object for previous build must remain strictly intact and unmutated"
    );

    // Verify new CAS object v2 was stored independently
    let hash_v2 = Sha256::digest_file(&dest).unwrap();
    assert_ne!(hash_v1, hash_v2, "v1 and v2 hashes must differ");
    let obj_path_v2 = cas.object_path(&hash_v2);
    assert!(
        obj_path_v2.is_file(),
        "CAS object v2 must exist: {:?}",
        obj_path_v2
    );

    // 3. Third compilation cycle: revert source back to v1 (triggering CAS cache hit / restore)
    fs::write(&main_rs, "fn main() { println!(\"binary_output_v1\"); }\n").unwrap();

    let (_plan3, res3) = build(&opt).expect("Build v3 must succeed");
    assert!(
        res3.success,
        "Build v3 failed: exit_code={}, stderr={}",
        res3.exit_code, res3.stderr
    );

    // Verify executions output v1 restored from cache
    let out_dest_v3 = std::process::Command::new(&dest).output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out_dest_v3.stdout).trim(),
        "binary_output_v1"
    );
    let out_alt_v3 = std::process::Command::new(&alt_path).output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out_alt_v3.stdout).trim(),
        "binary_output_v1"
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let dest_meta_v3 = fs::metadata(&dest).unwrap();
        let alt_meta_v3 = fs::metadata(&alt_path).unwrap();
        let cas_meta_v1 = fs::metadata(&obj_path_v1).unwrap();

        assert_eq!(
            dest_meta_v3.ino(),
            alt_meta_v3.ino(),
            "dest and alt_path after cache restore must share the same inode"
        );
        assert_ne!(
            dest_meta_v3.ino(),
            cas_meta_v1.ino(),
            "dest after cache restore must NOT share inode with CAS object"
        );
    }

    // Verify both CAS objects remain intact
    let cas_obj_v1_sha_final = Sha256::digest_file(&obj_path_v1).unwrap();
    assert_eq!(cas_obj_v1_sha_final, cas_obj_v1_sha);
    let cas_obj_v2_sha_final = Sha256::digest_file(&obj_path_v2).unwrap();
    assert_eq!(cas_obj_v2_sha_final, hash_v2);
}

#[test]
fn test_intra_workspace_library_aliases_and_chaining() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    let cache_dir = root.join("cache");
    let pkg_dir = root.join("my-lib-pkg");
    let src_dir = pkg_dir.join("src");
    fs::create_dir_all(&src_dir).unwrap();

    fs::write(
        pkg_dir.join("Cargo.toml"),
        r#"
[package]
name = "my-lib-pkg"
version = "0.1.0"
edition = "2021"
"#,
    )
    .unwrap();

    fs::write(
        pkg_dir.join("Cargo.lock"),
        r#"
version = 3

[[package]]
name = "my-lib-pkg"
version = "0.1.0"
"#,
    )
    .unwrap();

    fs::write(
        src_dir.join("lib.rs"),
        "pub fn add(a: i32, b: i32) -> i32 { a + b }\n",
    )
    .unwrap();

    let target_dir = pkg_dir.join("target").join("mitts");
    let opt = RustBuildOptions {
        manifest_path: Some(pkg_dir.join("Cargo.toml")),
        target_dir: Some(target_dir.clone()),
        cache_dir: Some(cache_dir.clone()),
        native: true,
        ..Default::default()
    };

    // 1. Initial build (compile from source)
    let (_plan1, res1) = build(&opt).expect("Build lib v1 must succeed");
    assert!(res1.success, "Build failed: {}", res1.stderr);

    let deps_dir = target_dir.join("debug").join("deps");
    let unhashed_rlib = deps_dir.join("libmy_lib_pkg.rlib");
    let unhashed_rmeta = deps_dir.join("libmy_lib_pkg.rmeta");

    assert!(unhashed_rlib.is_file(), "unhashed rlib must exist");
    assert!(unhashed_rmeta.is_file(), "unhashed rmeta must exist");

    // Locate hashed rlib and rmeta
    let mut hashed_rlib = None;
    let mut hashed_rmeta = None;
    for entry in fs::read_dir(&deps_dir).unwrap().flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with("libmy_lib_pkg-") && name.ends_with(".rlib") {
            hashed_rlib = Some(entry.path());
        } else if name.starts_with("libmy_lib_pkg-") && name.ends_with(".rmeta") {
            hashed_rmeta = Some(entry.path());
        }
    }

    let hashed_rlib = hashed_rlib.expect("hashed rlib must exist in deps");
    let hashed_rmeta = hashed_rmeta.expect("hashed rmeta must exist in deps");

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let u_rlib_meta = fs::metadata(&unhashed_rlib).unwrap();
        let h_rlib_meta = fs::metadata(&hashed_rlib).unwrap();
        assert_eq!(
            u_rlib_meta.ino(),
            h_rlib_meta.ino(),
            "lib<crate>.rlib and lib<crate>-<hash>.rlib must share inode on ext4 zero-copy"
        );

        let u_rmeta_meta = fs::metadata(&unhashed_rmeta).unwrap();
        let h_rmeta_meta = fs::metadata(&hashed_rmeta).unwrap();
        assert_eq!(
            u_rmeta_meta.ino(),
            h_rmeta_meta.ino(),
            "lib<crate>.rmeta and lib<crate>-<hash>.rmeta must share inode on ext4 zero-copy"
        );
    }

    // 2. Cache Hit / Restore (clean target directory, keep CAS cache intact)
    let _ = fs::remove_dir_all(&target_dir);

    let (_plan2, res2) = build(&opt).expect("Build lib v2 (cache hit) must succeed");
    assert!(res2.success);

    assert!(unhashed_rlib.is_file(), "unhashed rlib must exist after restore");
    assert!(hashed_rlib.is_file(), "hashed rlib must exist after restore");

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let u_rlib_meta_2 = fs::metadata(&unhashed_rlib).unwrap();
        let h_rlib_meta_2 = fs::metadata(&hashed_rlib).unwrap();
        assert_eq!(
            u_rlib_meta_2.ino(),
            h_rlib_meta_2.ino(),
            "lib<crate>.rlib and lib<crate>-<hash>.rlib must share inode after CAS restore"
        );
    }

    // 3. Repeated hit without clean (fast-path: identical inodes, no-op)
    let (_plan3, res3) = build(&opt).expect("Build lib v3 (repeated hit) must succeed");
    assert!(res3.success);

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let u_rlib_meta_3 = fs::metadata(&unhashed_rlib).unwrap();
        let h_rlib_meta_3 = fs::metadata(&hashed_rlib).unwrap();
        assert_eq!(
            u_rlib_meta_3.ino(),
            h_rlib_meta_3.ino(),
            "repeated cache hit must preserve identical inodes"
        );
    }
}

#[test]
fn test_intra_workspace_cas_isolation_with_writable_permissions_and_artifacts() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    let cas_dir = root.join("cas");
    let cas = ContentAddressedStore::new(cas_dir).unwrap();

    let ws_dir = root.join("workspace").join("target");
    fs::create_dir_all(&ws_dir).unwrap();

    // 1. Create a file and store it in CAS
    let src = ws_dir.join("temp_src.bin");
    fs::write(&src, b"cas object bytes").unwrap();
    let (hash, _) = cas.store_file(&src).unwrap();
    let cas_obj = cas.object_path(&hash);
    assert!(cas_obj.is_file());

    // Simulate "chmod -R u+rwx" on CAS (writable permissions 0o644 / 0o755)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&cas_obj, fs::Permissions::from_mode(0o644));
    }

    // Attempt link_intra_workspace from CAS object to workspace target
    let ws_dst1 = ws_dir.join("restored_obj.bin");
    link_intra_workspace(&cas_obj, &ws_dst1, &[&ws_dir], false).unwrap();
    assert!(ws_dst1.is_file());

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let cas_meta = fs::metadata(&cas_obj).unwrap();
        let dst_meta = fs::metadata(&ws_dst1).unwrap();
        assert_ne!(
            cas_meta.ino(),
            dst_meta.ino(),
            "CAS object even with 0644 mode must NEVER share inode with workspace"
        );
    }

    // 2. Create a named artifact in cas/artifacts/ and set to 0o644
    let cas_artifact = cas.named_artifact_path("my_crate", &hash, "rlib").unwrap();
    assert!(cas_artifact.is_file());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&cas_artifact, fs::Permissions::from_mode(0o644));
    }

    // Attempt link_intra_workspace from cas/artifacts to workspace target
    let ws_dst2 = ws_dir.join("restored_artifact.rlib");
    link_intra_workspace(&cas_artifact, &ws_dst2, &[&ws_dir], false).unwrap();
    assert!(ws_dst2.is_file());

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let art_meta = fs::metadata(&cas_artifact).unwrap();
        let dst_meta = fs::metadata(&ws_dst2).unwrap();
        assert_ne!(
            art_meta.ino(),
            dst_meta.ino(),
            "cas/artifacts artifact even with 0644 mode must NEVER share inode with workspace"
        );
    }
}


