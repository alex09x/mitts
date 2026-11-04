/*
 * Mitts Build — package verification tests
 * Copyright (c) 2026 Alexander Panasenko
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */
use super::{prepared_archive, reject_symlinks};
use std::io::Write;

#[test]
fn prepared_archive_requires_identical_manifest() {
    let temp = tempfile::tempdir().unwrap();
    let manifest = temp.path().join("Cargo.toml");
    let source = "[package]\nname='demo'\nversion='1.0.0'\n";
    std::fs::write(&manifest, source).unwrap();
    let metadata = serde_json::json!({"name": "demo", "vers": "1.0.0"});
    let path = temp.path().join("demo.crate");
    for (archive_manifest, accepted) in [
        (source, true),
        (
            "[package]\nname='demo'\nversion='1.0.0'\ndescription='changed'\n",
            false,
        ),
    ] {
        let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        let mut archive = tar::Builder::new(encoder);
        let mut header = tar::Header::new_gnu();
        header.set_size(archive_manifest.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        archive
            .append_data(
                &mut header,
                "demo-1.0.0/Cargo.toml.orig",
                archive_manifest.as_bytes(),
            )
            .unwrap();
        let mut normalized = tar::Header::new_gnu();
        normalized.set_size(source.len() as u64);
        normalized.set_mode(0o644);
        normalized.set_cksum();
        archive
            .append_data(&mut normalized, "demo-1.0.0/Cargo.toml", source.as_bytes())
            .unwrap();
        let bytes = archive.into_inner().unwrap().finish().unwrap();
        std::fs::File::create(&path)
            .unwrap()
            .write_all(&bytes)
            .unwrap();
        assert_eq!(
            prepared_archive(&path, &manifest, &metadata).is_ok(),
            accepted
        );
    }
}

#[test]
fn prepared_archive_rejects_oversized_unpacked_entry() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("oversized.crate");
    let mut header = tar::Header::new_gnu();
    header.set_path("demo-1.0.0/huge").unwrap();
    header.set_size(513 * 1024 * 1024);
    header.set_mode(0o644);
    header.set_cksum();
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(header.as_bytes()).unwrap();
    std::fs::write(&path, encoder.finish().unwrap()).unwrap();
    let metadata = serde_json::json!({"name": "demo", "vers": "1.0.0"});
    let error = prepared_archive(&path, &temp.path().join("Cargo.toml"), &metadata).unwrap_err();
    assert!(error.to_string().contains("unpacked crate is too large"));
}

#[cfg(unix)]
#[test]
fn verification_rejects_source_symlink_cycles() {
    let temp = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(".", temp.path().join("loop")).unwrap();
    assert!(reject_symlinks(temp.path()).is_err());
}
