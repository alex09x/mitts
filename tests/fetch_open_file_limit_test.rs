/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Fetching many crates under the open-file limit a macOS terminal starts with (256).
//! Cargo fetches there; Mitts ran out of descriptors ("Too many open files"): every
//! downloaded crate waiting to be unpacked held its lock file open, on top of 64 download
//! connections. The registry runs in a child process so its sockets do not count here.

#![cfg(unix)]

use flate2::write::GzEncoder;
use flate2::Compression;
use mitts::adapter::rust::fetcher::{CrateFetcher, FetchOptions};
use mitts::adapter::rust::lockfile::{CargoLock, CargoLockPackage};
use mitts::core::source_store::SourceStore;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const CRATES: usize = 300;
const FILES_PER_CRATE: usize = 40;
const OPEN_FILES: libc::rlim_t = 256;
const CHILD: &str = "MITTS_TEST_FETCH_FD_REGISTRY";

fn crate_archive(name: &str) -> Vec<u8> {
    let mut enc = GzEncoder::new(Vec::new(), Compression::fast());
    {
        let mut tar = tar::Builder::new(&mut enc);
        let mut add = |path: String, body: &[u8]| {
            let mut header = tar::Header::new_gnu();
            header.set_size(body.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append_data(&mut header, path, body).unwrap();
        };
        let manifest = format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\n");
        add(format!("{name}-0.1.0/Cargo.toml"), manifest.as_bytes());
        for i in 0..FILES_PER_CRATE {
            add(format!("{name}-0.1.0/src/m{i}.rs"), b"pub fn f() {}\n");
        }
        tar.finish().unwrap();
    }
    enc.finish().unwrap()
}

/// The child: a registry answering `/<name>/<name>-0.1.0.crate`, one thread per connection.
/// Its port goes to `port_file` (stdout also carries the test harness's own lines).
fn serve_registry(port_file: &std::path::Path) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port().to_string();
    std::fs::write(port_file.with_extension("tmp"), &port).unwrap();
    std::fs::rename(port_file.with_extension("tmp"), port_file).unwrap();
    for stream in listener.incoming() {
        let Ok(mut stream) = stream else { continue };
        std::thread::spawn(move || {
            let mut request = Vec::new();
            let mut buf = [0u8; 1024];
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                match stream.read(&mut buf) {
                    Ok(0) | Err(_) => return,
                    Ok(n) => request.extend_from_slice(&buf[..n]),
                }
            }
            let line = String::from_utf8_lossy(&request);
            let path = line.split_whitespace().nth(1).unwrap_or("/");
            let name = path.trim_start_matches('/').split('/').next().unwrap_or("");
            let body = crate_archive(name);
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(&body);
        });
    }
}

fn set_open_files(soft: libc::rlim_t) -> libc::rlimit {
    let mut old = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    assert_eq!(unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut old) }, 0);
    // Linux refuses any hard limit above `fs.nr_open`, even an unchanged one: a container
    // can be started with a hard limit (1048576) over the host's `nr_open` (1000000).
    let nr_open = std::fs::read_to_string("/proc/sys/fs/nr_open")
        .ok()
        .and_then(|s| s.trim().parse::<libc::rlim_t>().ok())
        .unwrap_or(old.rlim_max);
    let hard = old.rlim_max.min(nr_open);
    let new = libc::rlimit {
        rlim_cur: soft.min(hard),
        rlim_max: hard,
    };
    old.rlim_max = hard;
    old.rlim_cur = old.rlim_cur.min(hard);
    assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &new) }, 0);
    old
}

#[test]
fn fetching_many_crates_fits_in_the_open_file_limit_of_a_macos_terminal() {
    if let Some(port_file) = std::env::var_os(CHILD) {
        serve_registry(std::path::Path::new(&port_file));
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let port_file = temp.path().join("registry.port");
    let mut registry = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "fetching_many_crates_fits_in_the_open_file_limit_of_a_macos_terminal",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD, &port_file)
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let started = Instant::now();
    while !port_file.exists() {
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "registry did not start"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let port: u16 = std::fs::read_to_string(&port_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();

    let cargo_home = temp.path().join("cargo-home");
    std::fs::create_dir_all(&cargo_home).unwrap();
    std::env::set_var("CARGO_HOME", &cargo_home);
    std::env::set_var("MITTS_CRATES_IO_URL", format!("http://127.0.0.1:{port}"));

    let lock = CargoLock {
        version: 3,
        packages: (0..CRATES)
            .map(|i| CargoLockPackage {
                name: format!("fdcrate{i}"),
                version: "0.1.0".to_string(),
                source: Some("registry+https://github.com/rust-lang/crates.io-index".to_string()),
                checksum: None,
                dependencies: Vec::new(),
            })
            .collect(),
    };
    let store = SourceStore::new(temp.path().join("store")).unwrap();
    let fetcher = CrateFetcher::new(store.clone());

    let old = set_open_files(OPEN_FILES);
    let result = fetcher.fetch_all(&lock, &FetchOptions::default());
    unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &old) };
    let _ = registry.kill();
    let _ = registry.wait();

    let result = result.expect("fetch under a 256 open-file limit");
    assert_eq!(result.fetched, CRATES);
    for i in [0, CRATES / 2, CRATES - 1] {
        assert!(store.has_source(&format!("fdcrate{i}"), "0.1.0"));
    }
}
