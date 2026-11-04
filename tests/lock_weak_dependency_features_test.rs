/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A weak dependency feature, `dep?/feat`, does not enable an optional dependency in a build,
//! but Cargo's resolver, and therefore Cargo.lock, still lists that dependency with the feature
//! it was asked for. A lockfile written without the edge is rejected by `cargo --locked`.
//!
//! The fixture mirrors gimli 0.31: `std = ["extra?/std"]` with `extra` optional, which is what
//! made the lock of `mitts fetch` miss `fallible-iterator` and `stable_deref_trait`.

#![cfg(unix)]

use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

fn crate_archive(name: &str, version: &str, manifest_tail: &str, body: &str) -> (Vec<u8>, String) {
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    {
        let mut tar = tar::Builder::new(&mut gz);
        let mut add = |path: String, data: &str| {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append_data(&mut header, path, data.as_bytes()).unwrap();
        };
        add(
            format!("{name}-{version}/Cargo.toml"),
            &format!(
                "[package]\nname = \"{name}\"\nversion = \"{version}\"\nedition = \"2021\"\n\n{manifest_tail}\n"
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

/// A dependency record of the registry index.
fn dep(name: &str, optional: bool) -> String {
    format!(
        "{{\"name\":\"{name}\",\"req\":\"^0.1\",\"features\":[],\"optional\":{optional},\"default_features\":true,\"target\":null,\"kind\":\"normal\"}}"
    )
}

/// An index line. Features written with `dep:` or `?` belong in `features2` with `"v":2`, as
/// crates.io publishes them.
fn entry(name: &str, deps: &[String], features2: &str, sum: &str) -> Vec<u8> {
    format!(
        "{{\"name\":\"{name}\",\"vers\":\"0.1.0\",\"deps\":[{}],\"cksum\":\"{sum}\",\"features\":{{}},\"features2\":{features2},\"v\":2,\"yanked\":false}}\n",
        deps.join(",")
    )
    .into_bytes()
}

/// `holder` has two optional dependencies: `extra`, named by the weak feature `std`, and
/// `unused`, named only by `unused-feat`, which nothing enables.
fn registry() -> String {
    let holder_tail = "[dependencies]\nextra = { version = \"0.1\", optional = true }\n\
         unused = { version = \"0.1\", optional = true }\n\n\
         [features]\nstd = [\"extra?/std\"]\nunused-feat = [\"dep:unused\"]";
    let (holder, holder_sum) =
        crate_archive("holder", "0.1.0", holder_tail, "pub fn v() -> u32 { 7 }\n");
    let (extra, extra_sum) = crate_archive(
        "extra",
        "0.1.0",
        "[features]\nstd = []",
        "pub fn v() -> u32 { 1 }\n",
    );
    let (unused, unused_sum) = crate_archive("unused", "0.1.0", "", "pub fn v() -> u32 { 2 }\n");

    let mut routes: HashMap<String, Vec<u8>> = HashMap::new();
    routes.insert(
        format!("/index/{}", index_path("holder")),
        entry(
            "holder",
            &[dep("extra", true), dep("unused", true)],
            "{\"std\":[\"extra?/std\"],\"unused-feat\":[\"dep:unused\"]}",
            &holder_sum,
        ),
    );
    routes.insert(
        format!("/index/{}", index_path("extra")),
        entry("extra", &[], "{}", &extra_sum),
    );
    routes.insert(
        format!("/index/{}", index_path("unused")),
        entry("unused", &[], "{}", &unused_sum),
    );
    routes.insert("/dl/holder/0.1.0/download".to_string(), holder);
    routes.insert("/dl/extra/0.1.0/download".to_string(), extra);
    routes.insert("/dl/unused/0.1.0/download".to_string(), unused);

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
            let request = String::from_utf8_lossy(&buf[..n]).to_string();
            let path = request.split_whitespace().nth(1).unwrap_or("/").to_string();
            let (status, body) = match routes.get(&path) {
                Some(body) => ("200 OK", body.clone()),
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

fn write_project(dir: &Path, url: &str, features: &str) {
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::create_dir_all(dir.join(".cargo")).unwrap();
    fs::write(
        dir.join(".cargo/config.toml"),
        format!(
            "[source.crates-io]\nreplace-with = \"mirror\"\n[source.mirror]\nregistry = \"sparse+{url}/index/\"\n"
        ),
    )
    .unwrap();
    fs::write(
        dir.join("Cargo.toml"),
        format!(
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
             [dependencies]\nholder = {{ version = \"0.1\", default-features = false, features = [{features}] }}\n"
        ),
    )
    .unwrap();
    fs::write(
        dir.join("src/main.rs"),
        "fn main() { println!(\"{}\", holder::v()); }\n",
    )
    .unwrap();
}

struct Resolved {
    holder_dependencies: Vec<String>,
    packages: Vec<String>,
    output: String,
}

fn fetch(features: &str) -> Resolved {
    let url = registry();
    let temp = TempDir::new().unwrap();
    let cargo_home = temp.path().join("cargo_home");
    fs::create_dir_all(&cargo_home).unwrap();
    let app = temp.path().join("app");
    write_project(&app, &url, features);
    let out = Command::new(env!("CARGO_BIN_EXE_mitts"))
        .arg("fetch")
        .arg("--manifest-path")
        .arg(app.join("Cargo.toml"))
        .arg("--cache-dir")
        .arg(temp.path().join("cache"))
        .env("CI", "true")
        .env("CARGO_HOME", &cargo_home)
        .env("MITTS_SPARSE_INDEX_URL", "http://127.0.0.1:9")
        .env("MITTS_CRATES_IO_URL", "http://127.0.0.1:9")
        .env_remove("MITTS_CACHE_DIR")
        .output()
        .expect("run mitts fetch");
    let output = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "mitts fetch failed:\n{output}");

    let lock: toml::Value =
        toml::from_str(&fs::read_to_string(app.join("Cargo.lock")).unwrap()).unwrap();
    let all = lock["package"].as_array().unwrap();
    let names = |value: Option<&toml::Value>| -> Vec<String> {
        value
            .and_then(toml::Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .map(|item| item.as_str().unwrap().to_string())
                    .collect()
            })
            .unwrap_or_default()
    };
    let holder = all
        .iter()
        .find(|package| package["name"].as_str() == Some("holder"))
        .expect("holder is locked");
    Resolved {
        holder_dependencies: names(holder.get("dependencies")),
        packages: all
            .iter()
            .map(|package| package["name"].as_str().unwrap().to_string())
            .collect(),
        output,
    }
}

#[test]
fn a_weak_feature_reference_keeps_the_optional_dependency_in_the_lock() {
    let resolved = fetch("\"std\"");
    assert!(
        resolved.holder_dependencies.contains(&"extra".to_string()),
        "`std = [\"extra?/std\"]` keeps the edge to `extra`, as Cargo.lock does: {:?}\n{}",
        resolved.holder_dependencies,
        resolved.output
    );
    assert!(resolved.packages.contains(&"extra".to_string()));
}

#[test]
fn an_optional_dependency_no_enabled_feature_names_stays_out_of_the_lock() {
    let resolved = fetch("\"std\"");
    assert!(
        !resolved.holder_dependencies.contains(&"unused".to_string()),
        "{:?}",
        resolved.holder_dependencies
    );
    assert!(!resolved.packages.contains(&"unused".to_string()));
}

#[test]
fn without_the_feature_neither_optional_dependency_is_locked() {
    let resolved = fetch("");
    assert!(
        resolved.holder_dependencies.is_empty(),
        "{:?}",
        resolved.holder_dependencies
    );
    assert!(!resolved.packages.contains(&"extra".to_string()));
}

#[test]
fn a_strong_dep_feature_still_locks_its_dependency() {
    let resolved = fetch("\"unused-feat\"");
    assert!(resolved.holder_dependencies.contains(&"unused".to_string()));
    assert!(
        !resolved.holder_dependencies.contains(&"extra".to_string()),
        "`std` is not enabled, so its weak reference does not count: {:?}",
        resolved.holder_dependencies
    );
}
