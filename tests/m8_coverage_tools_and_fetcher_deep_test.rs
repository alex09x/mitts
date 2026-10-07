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
use mitts::adapter::rust::lockfile::CargoLockPackage;
use mitts::adapter::rust::sparse_index::SparseIndexClient;
use mitts::cli::coverage::{handle_coverage, CoverageArgs};
use mitts::cli::coverage_tools::resolve_llvm_tool;
use mitts::core::source_store::SourceStore;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc::{channel, Sender};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;
use tar::Builder;
use tempfile::TempDir;

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn make_tar_gz(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut enc = GzEncoder::new(Vec::new(), Compression::default());
    {
        let mut tar = Builder::new(&mut enc);
        for (name, content) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(content.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append_data(&mut header, *name, *content).unwrap();
        }
        tar.finish().unwrap();
    }
    enc.finish().unwrap()
}

fn spawn_mock_server(response_body: Vec<u8>, status_line: &str) -> (String, Sender<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = channel();
    let status = status_line.to_string();
    thread::spawn(move || {
        listener.set_nonblocking(true).unwrap();
        while rx.try_recv().is_err() {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);
                let resp = format!(
                    "HTTP/1.1 {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    status,
                    response_body.len()
                );
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.write_all(&response_body);
                let _ = stream.flush();
            }
            thread::sleep(Duration::from_millis(5));
        }
    });
    (format!("http://{addr}"), tx)
}

#[test]
fn test_m8_coverage_tool_resolution_and_missing_errors() {
    let _guard = ENV_LOCK.lock().unwrap();

    // 1. resolve_llvm_tool for a nonexistent tool returns None
    assert!(resolve_llvm_tool("nonexistent_llvm_tool_xyz").is_none());

    // 2. handle_coverage when llvm-profdata is missing
    std::env::set_var("MITTS_LLVM_PROFDATA", "missing");
    let args = CoverageArgs::default();
    let code = handle_coverage(&args);
    assert_eq!(code, 1);
    std::env::remove_var("MITTS_LLVM_PROFDATA");

    // 3. handle_coverage when llvm-cov is missing
    std::env::set_var("MITTS_LLVM_COV", "missing");
    let code2 = handle_coverage(&args);
    assert_eq!(code2, 1);
    std::env::remove_var("MITTS_LLVM_COV");
}

#[test]
fn test_m8_fetcher_http_download_and_store_hit() {
    let _guard = ENV_LOCK.lock().unwrap();
    let orig_cargo_home = std::env::var("CARGO_HOME").ok();
    let temp = TempDir::new().unwrap();

    let empty_cargo = temp.path().join("empty_cargo");
    fs::create_dir_all(&empty_cargo).unwrap();
    std::env::set_var("CARGO_HOME", &empty_cargo);

    let store_dir = temp.path().join("store");
    let store = SourceStore::new(store_dir).unwrap();

    let crate_bytes = make_tar_gz(&[
        (
            "unique_dl_pkg-0.1.0/Cargo.toml",
            b"[package]\nname = \"unique_dl_pkg\"\nversion = \"0.1.0\"\n",
        ),
        ("unique_dl_pkg-0.1.0/src/lib.rs", b"pub fn hello() {}\n"),
    ]);

    let (server_url, stop_server) = spawn_mock_server(crate_bytes, "200 OK");
    std::env::set_var("MITTS_CRATES_IO_URL", &server_url);

    let fetcher = CrateFetcher::new(store);
    let pkg = CargoLockPackage {
        name: "unique_dl_pkg".to_string(),
        version: "0.1.0".to_string(),
        source: Some("registry+https://github.com/rust-lang/crates.io-index".to_string()),
        checksum: None,
        dependencies: Vec::new(),
    };

    // 1. Fetch package over HTTP
    let dir = fetcher
        .fetch_package(&pkg, &FetchOptions::default())
        .unwrap();
    assert!(dir.is_some());
    let unpacked = dir.unwrap();
    assert!(unpacked.join("Cargo.toml").is_file());

    // 2. Second fetch hits local store cache directly (store.has_package branch)
    let dir2 = fetcher
        .fetch_package(&pkg, &FetchOptions::default())
        .unwrap();
    assert_eq!(dir2, Some(unpacked));

    let _ = stop_server.send(());
    std::env::remove_var("MITTS_CRATES_IO_URL");
    if let Some(h) = orig_cargo_home {
        std::env::set_var("CARGO_HOME", h);
    } else {
        std::env::remove_var("CARGO_HOME");
    }
}

#[test]
fn test_m8_sparse_index_client_http_and_errors() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = TempDir::new().unwrap();
    let cache_dir = temp.path().join("index_cache");

    // 1. Server returns 500 error
    let (err_server_url, stop_err) =
        spawn_mock_server(b"Internal Error".to_vec(), "500 Internal Server Error");
    std::env::set_var("MITTS_SPARSE_INDEX_URL", &err_server_url);

    let client = SparseIndexClient::with_cache_dir(cache_dir.clone()).unwrap();
    let res = client.fetch_crate_versions("remote_pkg", false);
    assert!(res.is_err());
    let err_str = res.unwrap_err().to_string();
    assert!(err_str.contains("Failed to fetch sparse index") || err_str.contains("remote_pkg"));
    let _ = stop_err.send(());

    // 2. Server returns 200 with valid line-delimited json
    let json_lines = br#"{"name":"remote_pkg","vers":"0.1.0","deps":[],"cksum":"abc","features":{},"yanked":false}
{"name":"remote_pkg","vers":"0.2.0","deps":[],"cksum":"def","features":{},"yanked":false}
"#;
    let (ok_server_url, stop_ok) = spawn_mock_server(json_lines.to_vec(), "200 OK");
    std::env::set_var("MITTS_SPARSE_INDEX_URL", &ok_server_url);

    let client_ok = SparseIndexClient::with_cache_dir(cache_dir).unwrap();
    let entries = client_ok.fetch_crate_versions("remote_pkg", false).unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].vers, "0.1.0");
    assert_eq!(entries[1].vers, "0.2.0");

    let _ = stop_ok.send(());
    std::env::remove_var("MITTS_SPARSE_INDEX_URL");
}
