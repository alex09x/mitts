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
#[allow(unused_imports)]
use mitts::core::fs_detect::{detect_filesystem, FilesystemKind, ZeroCopyStrategy};
use mitts::core::metrics::{get_metrics_snapshot, reset_metrics};
use mitts::core::reflink::restore_cas_object;
use std::fs;
use tempfile::TempDir;

#[test]
fn test_runtime_filesystem_detection() {
    let cwd = std::env::current_dir().unwrap();
    let info = detect_filesystem(&cwd);

    assert!(!info.name.is_empty());
    #[cfg(target_os = "macos")]
    {
        assert_eq!(info.kind, FilesystemKind::Apfs);
        assert_eq!(info.optimal_strategy, ZeroCopyStrategy::CoWReflink);
        assert!(info.supports_cow);
        assert!(info.supports_hardlink);
    }
    #[cfg(target_os = "linux")]
    {
        assert!(info.supports_hardlink);
        assert!(matches!(
            info.optimal_strategy,
            ZeroCopyStrategy::CoWReflink | ZeroCopyStrategy::SafeHardlink
        ));
    }
}

#[test]
fn test_restore_cas_object_permissions_and_integrity() {
    std::env::set_var("MITTS_CAS_HARDLINK", "1");
    reset_metrics();

    let tmp = TempDir::new().unwrap();
    let cas_dir = tmp.path().join("cas");
    let ws_dir = tmp.path().join("workspace");
    fs::create_dir_all(&ws_dir).unwrap();

    let cas = ContentAddressedStore::new(cas_dir).unwrap();
    let original_payload = b"test payload for zero-copy artifact restoration\n";
    let (hash, size) = cas.store_bytes(original_payload).unwrap();
    assert_eq!(size, original_payload.len() as u64);

    // 1. Restore non-executable object
    let target_dest = ws_dir.join("libtest.rlib");
    restore_cas_object(&cas.object_path(&hash), &target_dest, false).unwrap();
    assert!(target_dest.is_file());
    assert_eq!(fs::read(&target_dest).unwrap(), original_payload);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&target_dest).unwrap().permissions().mode() & 0o777;
        // On APFS/CoW mode is 0o644; on hardlink mode is 0o444 (read-only)
        assert!(
            mode == 0o644 || mode == 0o444,
            "Mode must be 0o644 (CoW) or 0o444 (read-only hardlink), got 0o{:o}",
            mode
        );
    }

    // 2. Restore executable object
    let bin_dest = ws_dir.join("test_bin");
    restore_cas_object(&cas.object_path(&hash), &bin_dest, true).unwrap();
    assert!(bin_dest.is_file());
    assert_eq!(fs::read(&bin_dest).unwrap(), original_payload);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&bin_dest).unwrap().permissions().mode() & 0o777;
        // Executable must be runnable (+x): either 0o755 (CoW) or 0o555 (read-only hardlink)
        assert!(
            mode == 0o755 || mode == 0o555,
            "Executable mode must be 0o755 or 0o555, got 0o{:o}",
            mode
        );
    }

    // 3. Overwriting target via atomic staging must never mutate CAS object
    let new_tmp = ws_dir.join(".test_bin.new_tmp");
    fs::write(&new_tmp, b"mutated executable payload\n").unwrap();
    fs::rename(&new_tmp, &bin_dest).unwrap();

    assert_eq!(fs::read(&bin_dest).unwrap(), b"mutated executable payload\n");
    // CAS object must remain completely unmutated and verify cleanly!
    assert_eq!(
        fs::read(cas.object_path(&hash)).unwrap(),
        original_payload,
        "CAS object was mutated!"
    );
    assert!(
        cas.verify_object(&hash).unwrap(),
        "CAS object verification failed after workspace overwrite"
    );

    let (cloned, copied, hardlinked, _, _, _, _, _, _, _) = get_metrics_snapshot();
    assert!(
        cloned > 0 || hardlinked > 0 || copied > 0,
        "At least one transfer metric must be recorded"
    );
}
