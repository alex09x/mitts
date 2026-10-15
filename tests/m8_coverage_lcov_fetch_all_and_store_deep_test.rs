/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use flate2::write::GzEncoder;
use flate2::Compression;
use mitts::adapter::rust::fetcher::{CrateFetcher, FetchOptions};
use mitts::adapter::rust::lockfile::{CargoLock, CargoLockPackage};
use mitts::cli::coverage::{handle_coverage, CoverageArgs};
use mitts::core::source_store::SourceStore;
use std::fs;
use std::path::Path;
use std::sync::Mutex;
use tar::Builder;
use tempfile::TempDir;

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn make_tar_gz(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut enc = GzEncoder::new(Vec::new(), Compression::default());
    {
        let mut tar = Builder::new(&mut enc);
        for (name, content) in files {
            let mut header = tar::Header::new_gnu();
            header.set_path(name).unwrap();
            header.set_size(content.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append(&header, *content).unwrap();
        }
        tar.finish().unwrap();
    }
    enc.finish().unwrap()
}

#[test]
fn test_m8_coverage_lcov_and_threshold_pass() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    let manifest = root.join("Cargo.toml");
    fs::write(
        &manifest,
        r#"
[package]
name = "lcov_test_pkg"
version = "0.1.0"
edition = "2021"

[lib]
path = "src/lib.rs"
"#,
    )
    .unwrap();

    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir).unwrap();
    fs::write(
        src_dir.join("lib.rs"),
        r#"
pub fn add(a: i32, b: i32) -> i32 {
    a + b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add() {
        assert_eq!(add(2, 3), 5);
    }
}
"#,
    )
    .unwrap();

    let out_dir = root.join("cov_out");
    let args = CoverageArgs {
        manifest_path: Some(manifest),
        lcov: true,
        output_dir: Some(out_dir.clone()),
        fail_under_lines: Some(1.0),
        ..Default::default()
    };

    let exit_code = handle_coverage(&args);
    assert_eq!(exit_code, 0);

    let lcov_file = out_dir.join("lcov.info");
    assert!(
        lcov_file.is_file(),
        "lcov.info was not created at {lcov_file:?}"
    );
    let lcov_content = fs::read_to_string(&lcov_file).unwrap();
    assert!(
        lcov_content.contains("SF:"),
        "lcov.info content missing SF: record"
    );
}

#[test]
fn test_m8_fetcher_fetch_all_multi_package_parallel() {
    let _lock = ENV_LOCK.lock().unwrap();
    let temp = TempDir::new().unwrap();
    let cargo_home = temp.path().join("cargo_home");
    let old_cargo_home = std::env::var("CARGO_HOME").ok();
    std::env::set_var("CARGO_HOME", &cargo_home);

    let reg_src = cargo_home
        .join("registry")
        .join("src")
        .join("index.crates.io-1234abcd");
    fs::create_dir_all(&reg_src).unwrap();

    // Prepare 3 packages in local registry cache
    for name in &["par_pkg_a", "par_pkg_b", "par_pkg_c"] {
        let pkg_dir = reg_src.join(format!("{name}-1.0.0"));
        fs::create_dir_all(pkg_dir.join("src")).unwrap();
        fs::write(
            pkg_dir.join("Cargo.toml"),
            format!("[package]\nname = \"{name}\"\nversion = \"1.0.0\"\nedition = \"2021\"\n"),
        )
        .unwrap();
        fs::write(pkg_dir.join(".cargo-ok"), "{\"v\":1}").unwrap();
        fs::write(pkg_dir.join("src/lib.rs"), "pub fn ok() {}\n").unwrap();
    }

    let store = SourceStore::new(temp.path().join("sources")).unwrap();
    let fetcher = CrateFetcher::new(store.clone());

    let lock = CargoLock {
        version: 3,
        packages: vec![
            CargoLockPackage {
                name: "par_pkg_a".to_string(),
                version: "1.0.0".to_string(),
                source: Some("registry+https://github.com/rust-lang/crates.io-index".to_string()),
                checksum: None,
                dependencies: Vec::new(),
            },
            CargoLockPackage {
                name: "par_pkg_b".to_string(),
                version: "1.0.0".to_string(),
                source: Some("registry+https://github.com/rust-lang/crates.io-index".to_string()),
                checksum: None,
                dependencies: Vec::new(),
            },
            CargoLockPackage {
                name: "par_pkg_c".to_string(),
                version: "1.0.0".to_string(),
                source: Some("registry+https://github.com/rust-lang/crates.io-index".to_string()),
                checksum: None,
                dependencies: Vec::new(),
            },
        ],
    };

    let opt = FetchOptions {
        offline: true,
        locked: false,
        frozen: false,
    };

    let res = fetcher.fetch_all(&lock, &opt).unwrap();
    assert_eq!(res.total_packages, 3);
    assert_eq!(res.already_cached, 0);
    assert_eq!(res.fetched, 3);
    assert!(store.has_source("par_pkg_a", "1.0.0"));
    assert!(store.has_source("par_pkg_b", "1.0.0"));
    assert!(store.has_source("par_pkg_c", "1.0.0"));

    if let Some(ch) = old_cargo_home {
        std::env::set_var("CARGO_HOME", ch);
    } else {
        std::env::remove_var("CARGO_HOME");
    }
}

#[test]
fn test_m8_source_store_unpack_archive_deep_branches() {
    let _lock = ENV_LOCK.lock().unwrap();
    let temp = TempDir::new().unwrap();
    let store = SourceStore::new(temp.path().join("sources")).unwrap();

    // 1. Target directory already exists -> removed and replaced
    let target = store.package_dir("exist_pkg", "1.0.0");
    fs::create_dir_all(&target).unwrap();
    fs::write(target.join("stale.txt"), "old").unwrap();

    let archive_bytes = make_tar_gz(&[(
        "exist_pkg-1.0.0/Cargo.toml",
        b"[package]\nname = \"exist_pkg\"\nversion = \"1.0.0\"\n",
    )]);
    let unpacked = store
        .unpack_archive("exist_pkg", "1.0.0", &archive_bytes, "")
        .unwrap();
    assert_eq!(unpacked, target);
    assert!(!target.join("stale.txt").exists());
    assert!(target.join("Cargo.toml").is_file());

    // 2. Flat archive without {name}-{version} prefix directory
    let flat_bytes = make_tar_gz(&[
        (
            "Cargo.toml",
            b"[package]\nname = \"flat_pkg\"\nversion = \"1.0.0\"\n",
        ),
        ("src/lib.rs", b"pub fn f() {}\n"),
    ]);
    let flat_target = store
        .unpack_archive("flat_pkg", "1.0.0", &flat_bytes, "")
        .unwrap();
    assert!(flat_target.join("Cargo.toml").is_file());
    assert!(flat_target.join("src/lib.rs").is_file());

    // 3. Archive missing Cargo.toml -> returns error
    let nocargo_bytes = make_tar_gz(&[("README.md", b"# No Cargo.toml\n")]);
    let err = store.unpack_archive("nocargo_pkg", "1.0.0", &nocargo_bytes, "");
    assert!(err.is_err());
    assert!(err.unwrap_err().to_string().contains("missing Cargo.toml"));

    // 4. SourceStore::default_store with MITTS_CACHE_DIR
    let cache_dir = temp.path().join("mitts_cache");
    std::env::set_var("MITTS_CACHE_DIR", &cache_dir);
    let ds = SourceStore::default_store().unwrap();
    assert_eq!(ds.root(), Path::new(&cache_dir).join("sources"));
    std::env::remove_var("MITTS_CACHE_DIR");
}
