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
use mitts::core::hash::Sha256;
use mitts::core::source_store::{SourceStore, MITTS_OK_MARKER};
use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, Mutex};
use std::time::SystemTime;
use tempfile::TempDir;

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn make_crate_archive(name: &str, version: &str, lib_rs: &str) -> (Vec<u8>, String) {
    let mut enc = GzEncoder::new(Vec::new(), Compression::default());
    {
        let mut tar = tar::Builder::new(&mut enc);
        let prefix = format!("{}-{}", name, version);
        let toml = format!(
            "[package]\nname = \"{}\"\nversion = \"{}\"\nedition = \"2021\"\n",
            name, version
        );
        for (rel, bytes) in [
            (format!("{}/Cargo.toml", prefix), toml.into_bytes()),
            (format!("{}/src/lib.rs", prefix), lib_rs.as_bytes().to_vec()),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_path(&rel).unwrap();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append(&header, &bytes[..]).unwrap();
        }
        tar.finish().unwrap();
    }
    let bytes = enc.finish().unwrap();
    let cksum = Sha256::digest(&bytes);
    (bytes, cksum)
}

fn snapshot_tree(root: &Path) -> BTreeMap<PathBuf, (u64, Option<SystemTime>)> {
    let mut out = BTreeMap::new();
    fn walk(base: &Path, dir: &Path, out: &mut BTreeMap<PathBuf, (u64, Option<SystemTime>)>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let rel = path.strip_prefix(base).unwrap_or(&path).to_path_buf();
            let meta = fs::metadata(&path).unwrap();
            out.insert(
                rel,
                (
                    if meta.is_file() { meta.len() } else { 0 },
                    meta.modified().ok(),
                ),
            );
            if meta.is_dir() {
                walk(base, &path, out);
            }
        }
    }
    walk(root, root, &mut out);
    out
}

#[test]
fn test_t0_p0_1_git_dependency_missing_rejects_without_http() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = TempDir::new().unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let addr = listener.local_addr().unwrap();
    let http_hits = Arc::new(AtomicUsize::new(0));
    let hits_clone = Arc::clone(&http_hits);
    let stop = Arc::new(AtomicUsize::new(0));
    let stop_clone = Arc::clone(&stop);

    let server_handle = std::thread::spawn(move || {
        while stop_clone.load(Ordering::Relaxed) == 0 {
            if let Ok((mut stream, _)) = listener.accept() {
                hits_clone.fetch_add(1, Ordering::SeqCst);
                let mut buf = [0u8; 512];
                let _ = stream.read(&mut buf);
                let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
            } else {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
    });

    let orig_home = std::env::var_os("HOME");
    let orig_ch = std::env::var_os("CARGO_HOME");
    let orig_url = std::env::var_os("MITTS_CRATES_IO_URL");

    let empty_home = temp.path().join("empty_home");
    fs::create_dir_all(&empty_home).unwrap();
    std::env::set_var("HOME", &empty_home);
    std::env::set_var("CARGO_HOME", empty_home.join(".cargo"));
    std::env::set_var("MITTS_CRATES_IO_URL", format!("http://{}", addr));

    let store = SourceStore::new(temp.path().join("store")).unwrap();
    let fetcher = CrateFetcher::new(store);

    let git_pkg = CargoLockPackage {
        name: "foo".to_string(),
        version: "0.1.0".to_string(),
        source: Some("git+https://github.com/example/foo#abcdef1234567890".to_string()),
        checksum: None,
        dependencies: Vec::new(),
    };

    let err_single = fetcher.fetch_package(&git_pkg, &FetchOptions::default());
    assert!(err_single.is_err(), "Missing git dependency must fail");
    assert!(err_single
        .unwrap_err()
        .to_string()
        .contains("run `cargo fetch`"));

    let lock = CargoLock {
        version: 3,
        packages: vec![git_pkg],
    };
    let err_all = fetcher.fetch_all(&lock, &FetchOptions::default());
    assert!(err_all.is_err(), "fetch_all with missing git dep must fail");
    assert!(err_all
        .unwrap_err()
        .to_string()
        .contains("run `cargo fetch`"));

    stop.store(1, Ordering::Relaxed);
    let _ = server_handle.join();

    assert_eq!(
        http_hits.load(Ordering::SeqCst),
        0,
        "Zero HTTP requests may be sent to crates.io for git dependencies"
    );

    if let Some(h) = orig_home {
        std::env::set_var("HOME", h);
    }
    if let Some(ch) = orig_ch {
        std::env::set_var("CARGO_HOME", ch);
    } else {
        std::env::remove_var("CARGO_HOME");
    }
    if let Some(u) = orig_url {
        std::env::set_var("MITTS_CRATES_IO_URL", u);
    } else {
        std::env::remove_var("MITTS_CRATES_IO_URL");
    }
}

#[test]
fn test_t0_p0_2_cargo_registry_is_strictly_read_only_and_requires_cargo_ok() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = TempDir::new().unwrap();

    let fake_home = temp.path().join("home");
    let cargo_home = fake_home.join(".cargo");
    let reg_root = cargo_home.join("registry");
    let reg_src = reg_root
        .join("src")
        .join("index.crates.io-1949cf8c6b5b557f");
    let reg_cache = reg_root
        .join("cache")
        .join("index.crates.io-1949cf8c6b5b557f");
    fs::create_dir_all(&reg_src).unwrap();
    fs::create_dir_all(&reg_cache).unwrap();

    // 1. Seeded crate in Cargo src WITH .cargo-ok
    let marked_dir = reg_src.join("marked-pkg-0.1.0");
    fs::create_dir_all(marked_dir.join("src")).unwrap();
    fs::write(
        marked_dir.join("Cargo.toml"),
        "[package]\nname = \"marked-pkg\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(
        marked_dir.join("src/lib.rs"),
        "pub fn from_cargo_src() {}\n",
    )
    .unwrap();
    fs::write(marked_dir.join(".cargo-ok"), "{\"v\":1}").unwrap();

    // 2. Half-unpacked crate in Cargo src WITHOUT .cargo-ok (must be ignored by Mitts!)
    let unmarked_dir = reg_src.join("unmarked-pkg-0.1.0");
    fs::create_dir_all(unmarked_dir.join("src")).unwrap();
    fs::write(
        unmarked_dir.join("Cargo.toml"),
        "[package]\nname = \"unmarked-pkg\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(
        unmarked_dir.join("src/lib.rs"),
        "pub fn incomplete_cargo() {}\n",
    )
    .unwrap();

    // 3. Seeded .crate archive in Cargo cache for unmarked-pkg
    let (archive_bytes, cksum) =
        make_crate_archive("unmarked-pkg", "0.1.0", "pub fn from_valid_archive() {}\n");
    fs::write(reg_cache.join("unmarked-pkg-0.1.0.crate"), &archive_bytes).unwrap();

    let before_snapshot = snapshot_tree(&reg_root);

    let orig_home = std::env::var_os("HOME");
    let orig_ch = std::env::var_os("CARGO_HOME");
    let orig_mc = std::env::var_os("MITTS_CACHE_DIR");
    std::env::remove_var("MITTS_CACHE_DIR");
    std::env::set_var("HOME", &fake_home);
    std::env::set_var("CARGO_HOME", &cargo_home);

    let default_store = SourceStore::default_store().unwrap();
    assert_eq!(
        default_store.root(),
        fake_home.join(".cache").join("mitts").join("sources"),
        "Default SourceStore must use ~/.cache/mitts/sources and never ~/.cargo/registry"
    );

    let fetcher = CrateFetcher::new(default_store.clone());
    let lock = CargoLock {
        version: 3,
        packages: vec![
            CargoLockPackage {
                name: "marked-pkg".to_string(),
                version: "0.1.0".to_string(),
                source: Some("registry+https://github.com/rust-lang/crates.io-index".to_string()),
                checksum: None,
                dependencies: Vec::new(),
            },
            CargoLockPackage {
                name: "unmarked-pkg".to_string(),
                version: "0.1.0".to_string(),
                source: Some("registry+https://github.com/rust-lang/crates.io-index".to_string()),
                checksum: Some(cksum),
                dependencies: Vec::new(),
            },
        ],
    };

    let res = fetcher
        .fetch_all(
            &lock,
            &FetchOptions {
                offline: true,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(res.total_packages, 2);
    assert_eq!(res.fetched, 2);

    // Verify unmarked-pkg came from the valid archive, NOT the unmarked directory in registry/src
    let unpacked_unmarked = fs::read_to_string(
        default_store
            .package_dir("unmarked-pkg", "0.1.0")
            .join("src/lib.rs"),
    )
    .unwrap();
    assert_eq!(unpacked_unmarked, "pub fn from_valid_archive() {}\n");

    let after_snapshot = snapshot_tree(&reg_root);
    assert_eq!(
        before_snapshot, after_snapshot,
        "$CARGO_HOME/registry inventory and mtimes must not change after mitts fetch"
    );

    if let Some(mc) = orig_mc {
        std::env::set_var("MITTS_CACHE_DIR", mc);
    }
    if let Some(ch) = orig_ch {
        std::env::set_var("CARGO_HOME", ch);
    } else {
        std::env::remove_var("CARGO_HOME");
    }
    if let Some(h) = orig_home {
        std::env::set_var("HOME", h);
    }
}

#[test]
fn test_t0_p1a_interrupted_unpack_without_mitts_ok_is_recovered() {
    let temp = TempDir::new().unwrap();
    let store = SourceStore::new(temp.path().join("sources")).unwrap();

    // Simulate a killed process that wrote Cargo.toml and a partial file directly without .mitts-ok
    let pkg_dir = store.package_dir("interrupted", "0.2.0");
    fs::create_dir_all(pkg_dir.join("src")).unwrap();
    fs::write(
        pkg_dir.join("Cargo.toml"),
        "[package]\nname = \"interrupted\"\nversion = \"0.2.0\"\n",
    )
    .unwrap();
    fs::write(pkg_dir.join("src/partial.rs"), "corrupted half write").unwrap();

    assert!(
        !store.has_source("interrupted", "0.2.0"),
        "Directory without .mitts-ok must not be treated as ready"
    );

    let (archive, cksum) = make_crate_archive("interrupted", "0.2.0", "pub fn complete() {}\n");
    let out_dir = store
        .unpack_archive("interrupted", "0.2.0", &archive, &cksum)
        .unwrap();

    assert_eq!(out_dir, pkg_dir);
    assert!(store.has_source("interrupted", "0.2.0"));
    assert!(pkg_dir.join(MITTS_OK_MARKER).is_file());
    assert!(!pkg_dir.join("src/partial.rs").exists());
    assert_eq!(
        fs::read_to_string(pkg_dir.join("src/lib.rs")).unwrap(),
        "pub fn complete() {}\n"
    );
}

#[test]
fn test_t0_p1b_concurrent_12_workers_lock_free_atomic_publish() {
    let temp = TempDir::new().unwrap();
    let store = SourceStore::new(temp.path().join("sources")).unwrap();

    let crates: Vec<(String, String, Vec<u8>, String)> = (0..5)
        .map(|i| {
            let name = format!(" swarm_crate_{} ", i).trim().to_string();
            let ver = "1.0.0".to_string();
            let body = format!("pub const ID: usize = {};\n", i);
            let (bytes, cksum) = make_crate_archive(&name, &ver, &body);
            (name, ver, bytes, cksum)
        })
        .collect();

    let workers = 12;
    let barrier = Arc::new(Barrier::new(workers));

    std::thread::scope(|s| {
        let mut handles = Vec::with_capacity(workers);
        for _ in 0..workers {
            let store_ref = store.clone();
            let crates_ref = &crates;
            let bar_ref = Arc::clone(&barrier);
            handles.push(s.spawn(move || {
                bar_ref.wait();
                for (name, ver, bytes, cksum) in crates_ref {
                    let dir = store_ref
                        .unpack_archive(name, ver, bytes, cksum)
                        .expect("concurrent atomic unpack must succeed");
                    assert!(dir.join(MITTS_OK_MARKER).is_file());
                    assert!(dir.join("Cargo.toml").is_file());
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
    });

    for (i, (name, ver, _, _)) in crates.iter().enumerate() {
        assert!(store.has_source(name, ver));
        let pkg_dir = store.package_dir(name, ver);
        let content = fs::read_to_string(pkg_dir.join("src/lib.rs")).unwrap();
        assert_eq!(content, format!("pub const ID: usize = {};\n", i));
    }

    // Verify no leftover .staging or .trash directories in .locks
    let leftovers: Vec<_> = fs::read_dir(store.locks_dir())
        .unwrap()
        .flatten()
        .filter(|e| {
            let s = e.file_name().to_string_lossy().to_string();
            s.ends_with(".staging") || s.ends_with(".trash")
        })
        .collect();
    assert!(
        leftovers.is_empty(),
        "Expected zero leftover staging/trash dirs, found {:?}",
        leftovers
    );
}
