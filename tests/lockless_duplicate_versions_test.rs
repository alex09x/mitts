/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Two versions of one crate in a lockless resolution: tonic's graph carries strum 0.27 and
//! 0.28. The lock Mitts wrote listed the dependency as a bare `strum`, which its own loader
//! then refused as ambiguous, so every worktree without a checked-in Cargo.lock failed before
//! compiling. Cargo writes `strum 0.27.2`; so does Mitts now.

#![cfg(unix)]

use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

/// A `.crate` archive with the given manifest tail and library body, with its sha256.
fn crate_archive(name: &str, version: &str, deps: &str, body: &str) -> (Vec<u8>, String) {
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    {
        let mut tar = tar::Builder::new(&mut gz);
        let mut add = |path: String, data: &str| {
            let mut h = tar::Header::new_gnu();
            h.set_size(data.len() as u64);
            h.set_mode(0o644);
            h.set_cksum();
            tar.append_data(&mut h, path, data.as_bytes()).unwrap();
        };
        add(
            format!("{name}-{version}/Cargo.toml"),
            &format!(
                "[package]\nname = \"{name}\"\nversion = \"{version}\"\nedition = \"2021\"\n\n[dependencies]\n{deps}\n"
            ),
        );
        add(format!("{name}-{version}/src/lib.rs"), body);
        tar.finish().unwrap();
    }
    let bytes = gz.finish().unwrap();
    let sum = format!("{:x}", Sha256::digest(&bytes));
    (bytes, sum)
}

fn index_path(name: &str) -> String {
    match name.len() {
        1 => format!("1/{name}"),
        2 => format!("2/{name}"),
        3 => format!("3/{}/{name}", &name[..1]),
        _ => format!("{}/{}/{name}", &name[..2], &name[2..4]),
    }
}

fn entry(name: &str, version: &str, deps: &str, sum: &str) -> String {
    format!(
        "{{\"name\":\"{name}\",\"vers\":\"{version}\",\"deps\":[{deps}],\"cksum\":\"{sum}\",\"features\":{{}},\"yanked\":false}}\n"
    )
}

fn dep(name: &str, req: &str) -> String {
    format!(
        "{{\"name\":\"{name}\",\"req\":\"{req}\",\"features\":[],\"optional\":false,\"default_features\":true,\"target\":null,\"kind\":\"normal\"}}"
    )
}

/// A registry with `shared 0.1.0` and `shared 0.2.0`, `one` needing `^0.1`, `two` needing `^0.2`.
fn registry() -> String {
    let (s1, s1_sum) = crate_archive("shared", "0.1.0", "", "pub fn v() -> u32 { 1 }\n");
    let (s2, s2_sum) = crate_archive("shared", "0.2.0", "", "pub fn v() -> u32 { 2 }\n");
    let (one, one_sum) = crate_archive(
        "one",
        "0.1.0",
        "shared = \"0.1\"",
        "pub fn v() -> u32 { shared::v() }\n",
    );
    let (two, two_sum) = crate_archive(
        "two",
        "0.1.0",
        "shared = \"0.2\"",
        "pub fn v() -> u32 { shared::v() * 10 }\n",
    );
    let mut routes = HashMap::new();
    routes.insert(
        format!("/index/{}", index_path("shared")),
        (entry("shared", "0.1.0", "", &s1_sum) + &entry("shared", "0.2.0", "", &s2_sum)).into_bytes(),
    );
    routes.insert(
        format!("/index/{}", index_path("one")),
        entry("one", "0.1.0", &dep("shared", "^0.1"), &one_sum).into_bytes(),
    );
    routes.insert(
        format!("/index/{}", index_path("two")),
        entry("two", "0.1.0", &dep("shared", "^0.2"), &two_sum).into_bytes(),
    );
    routes.insert("/dl/shared/0.1.0/download".to_string(), s1);
    routes.insert("/dl/shared/0.2.0/download".to_string(), s2);
    routes.insert("/dl/one/0.1.0/download".to_string(), one);
    routes.insert("/dl/two/0.1.0/download".to_string(), two);
    let probe = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = probe.local_addr().unwrap().port();
    drop(probe);
    let base = format!("http://127.0.0.1:{port}");
    routes.insert(
        "/index/config.json".to_string(),
        format!("{{\"dl\":\"{base}/dl\",\"api\":\"{base}\",\"auth-required\":false}}").into_bytes(),
    );
    let listener = TcpListener::bind(("127.0.0.1", port)).unwrap();
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut buf = [0u8; 8192];
            let n = stream.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            let path = req.split_whitespace().nth(1).unwrap_or("/").to_string();
            let (status, body) = match routes.get(&path) {
                Some(b) => ("200 OK", b.clone()),
                None => ("404 Not Found", Vec::new()),
            };
            let _ = write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(&body);
        }
    });
    base
}

fn write_project(dir: &Path, url: &str) {
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::create_dir_all(dir.join(".cargo")).unwrap();
    fs::write(
        dir.join(".cargo/config.toml"),
        format!("[source.crates-io]\nreplace-with = \"mirror\"\n[source.mirror]\nregistry = \"sparse+{url}/index/\"\n"),
    )
    .unwrap();
    fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\none = \"0.1\"\ntwo = \"0.1\"\n",
    )
    .unwrap();
    fs::write(
        dir.join("src/main.rs"),
        "fn main() { println!(\"{}\", one::v() + two::v()); }\n",
    )
    .unwrap();
}

#[test]
fn a_lockless_resolution_with_two_versions_of_a_crate_writes_qualified_dependency_specs() {
    let url = registry();
    let temp = TempDir::new().unwrap();
    let cargo_home = temp.path().join("cargo_home");
    fs::create_dir_all(&cargo_home).unwrap();
    let app = temp.path().join("app");
    write_project(&app, &url);
    let cache = temp.path().join("cache");
    let run = |args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_mitts"))
            .args(args)
            .arg("--manifest-path")
            .arg(app.join("Cargo.toml"))
            .arg("--cache-dir")
            .arg(&cache)
            .env("CI", "true")
            .env("CARGO_HOME", &cargo_home)
            .env("MITTS_SPARSE_INDEX_URL", "http://127.0.0.1:9")
            .env("MITTS_CRATES_IO_URL", "http://127.0.0.1:9")
            .env_remove("MITTS_CACHE_DIR")
            .output()
            .expect("run mitts");
        (
            out.status.success(),
            format!(
                "{}\n{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            ),
        )
    };
    // No Cargo.lock: Mitts resolves and writes one, then reads it back for the plan.
    let (ok, text) = run(&["build"]);
    assert!(ok, "build with two versions of `shared`:\n{text}");
    let lock = fs::read_to_string(app.join("Cargo.lock")).unwrap();
    assert!(lock.contains("\"shared 0.1.0\""), "one depends on shared 0.1.0:\n{lock}");
    assert!(lock.contains("\"shared 0.2.0\""), "two depends on shared 0.2.0:\n{lock}");
    let out = Command::new(app.join("target/mitts/debug/app")).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "21");
    // A second run reads the written lock: the ambiguity must not come back.
    let (ok, text) = run(&["check"]);
    assert!(ok, "check from the written lock:\n{text}");
}
