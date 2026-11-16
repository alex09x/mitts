/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Issue 140: adding one dependency to a workspace rewrote its whole Cargo.lock. Unrelated
//! packages moved to their newest version (rsa 0.10.0-rc.19 -> 0.10.0, which dropped the
//! `sha2` feature a dependent needs), `version = 4` became 3, and a crate depending on two
//! versions of one package (serde_with: schemars 0.9 and 1) kept one, written bare.
//!
//! Each test serves a small sparse registry and runs `mitts fetch` on a project whose
//! Cargo.lock, if any, is written by the test.

#![cfg(unix)]

use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

const CRATES_IO: &str = "registry+https://github.com/rust-lang/crates.io-index";

/// One published version: `deps` are index dependency records, `features` the feature table.
struct Published {
    name: &'static str,
    vers: &'static str,
    deps: Vec<String>,
    features: &'static str,
}

fn version(name: &'static str, vers: &'static str) -> Published {
    Published {
        name,
        vers,
        deps: Vec::new(),
        features: "{}",
    }
}

/// An index dependency record; `package` renames it.
fn dep(name: &str, package: Option<&str>, req: &str, features: &[&str]) -> String {
    let package = package
        .map(|p| format!(",\"package\":\"{p}\""))
        .unwrap_or_default();
    let features = features
        .iter()
        .map(|f| format!("\"{f}\""))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "{{\"name\":\"{name}\"{package},\"req\":\"{req}\",\"features\":[{features}],\"optional\":false,\"default_features\":true,\"target\":null,\"kind\":\"normal\"}}"
    )
}

fn index_path(name: &str) -> String {
    match name.len() {
        1 => format!("1/{name}"),
        2 => format!("2/{name}"),
        3 => format!("3/{}/{name}", &name[..1]),
        _ => format!("{}/{}/{name}", &name[..2], &name[2..4]),
    }
}

fn archive(name: &str, vers: &str) -> (Vec<u8>, String) {
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    {
        let mut tar = tar::Builder::new(&mut gz);
        for (path, data) in [
            (
                format!("{name}-{vers}/Cargo.toml"),
                format!("[package]\nname = \"{name}\"\nversion = \"{vers}\"\nedition = \"2021\"\n"),
            ),
            (format!("{name}-{vers}/src/lib.rs"), String::new()),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append_data(&mut header, path, data.as_bytes()).unwrap();
        }
        tar.finish().unwrap();
    }
    let bytes = gz.finish().unwrap();
    let sum = format!("{:x}", Sha256::digest(&bytes));
    (bytes, sum)
}

/// Serves the versions as a sparse registry; returns its base URL.
fn registry(published: Vec<Published>) -> String {
    let mut routes: HashMap<String, Vec<u8>> = HashMap::new();
    let mut lines: HashMap<&str, String> = HashMap::new();
    for p in &published {
        let (bytes, sum) = archive(p.name, p.vers);
        routes.insert(format!("/dl/{}/{}/download", p.name, p.vers), bytes);
        lines.entry(p.name).or_default().push_str(&format!(
            "{{\"name\":\"{}\",\"vers\":\"{}\",\"deps\":[{}],\"cksum\":\"{sum}\",\"features\":{},\"yanked\":false}}\n",
            p.name,
            p.vers,
            p.deps.join(","),
            p.features
        ));
    }
    for (name, text) in lines {
        routes.insert(format!("/index/{}", index_path(name)), text.into_bytes());
    }
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    routes.insert(
        "/index/config.json".to_string(),
        format!("{{\"dl\":\"{base}/dl\",\"api\":\"{base}\",\"auth-required\":false}}").into_bytes(),
    );
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

/// A project `app` with `dependencies` (TOML lines) and, if given, an existing Cargo.lock.
fn project(dir: &Path, url: &str, dependencies: &str, lock: Option<&str>) {
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::create_dir_all(dir.join(".cargo")).unwrap();
    fs::write(
        dir.join(".cargo/config.toml"),
        format!("[source.crates-io]\nreplace-with = \"mirror\"\n[source.mirror]\nregistry = \"sparse+{url}/index/\"\n"),
    )
    .unwrap();
    fs::write(
        dir.join("Cargo.toml"),
        format!("[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n{dependencies}\n"),
    )
    .unwrap();
    fs::write(dir.join("src/main.rs"), "fn main() {}\n").unwrap();
    if let Some(lock) = lock {
        fs::write(dir.join("Cargo.lock"), lock).unwrap();
    }
}

/// An existing lock (format 4) pinning `pins` as (name, version, dependencies).
fn lock_with(app_deps: &[&str], pins: &[(&str, &str, &[&str])]) -> String {
    let mut text =
        String::from("version = 4\n\n[[package]]\nname = \"app\"\nversion = \"0.1.0\"\n");
    text.push_str(&format!(
        "dependencies = [{}]\n",
        app_deps
            .iter()
            .map(|d| format!("\"{d}\""))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    for (name, vers, deps) in pins {
        text.push_str(&format!(
            "\n[[package]]\nname = \"{name}\"\nversion = \"{vers}\"\nsource = \"{CRATES_IO}\"\nchecksum = \"{}\"\n",
            archive(name, vers).1
        ));
        if !deps.is_empty() {
            text.push_str(&format!(
                "dependencies = [{}]\n",
                deps.iter()
                    .map(|d| format!("\"{d}\""))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }
    text
}

struct Locked {
    format: i64,
    /// (name, version) of every registry package.
    packages: Vec<(String, String)>,
    /// Dependencies listed for each package, by "name version".
    edges: HashMap<String, Vec<String>>,
}

fn fetch(published: Vec<Published>, dependencies: &str, lock: Option<&str>) -> Locked {
    let url = registry(published);
    let temp = TempDir::new().unwrap();
    let app = temp.path().join("app");
    project(&app, &url, dependencies, lock);
    let out = Command::new(env!("CARGO_BIN_EXE_mitts"))
        .args(["fetch", "--manifest-path"])
        .arg(app.join("Cargo.toml"))
        .arg("--cache-dir")
        .arg(temp.path().join("cache"))
        .env("CI", "true")
        .env("CARGO_HOME", temp.path().join("cargo_home"))
        .env("MITTS_SPARSE_INDEX_URL", "http://127.0.0.1:9")
        .env("MITTS_CRATES_IO_URL", "http://127.0.0.1:9")
        .env("MITTS_NET_RETRY", "0")
        .env_remove("MITTS_CACHE_DIR")
        .output()
        .unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "mitts fetch failed:\n{text}");
    let lock: toml::Value =
        toml::from_str(&fs::read_to_string(app.join("Cargo.lock")).unwrap()).unwrap();
    let mut packages = Vec::new();
    let mut edges = HashMap::new();
    for p in lock["package"].as_array().unwrap() {
        let name = p["name"].as_str().unwrap().to_string();
        let vers = p["version"].as_str().unwrap().to_string();
        let deps = p
            .get("dependencies")
            .and_then(|d| d.as_array())
            .map(|d| d.iter().map(|x| x.as_str().unwrap().to_string()).collect())
            .unwrap_or_default();
        edges.insert(format!("{name} {vers}"), deps);
        if name != "app" {
            packages.push((name, vers));
        }
    }
    packages.sort();
    Locked {
        format: lock["version"].as_integer().unwrap_or(0),
        packages,
        edges,
    }
}

fn pkg(name: &str, vers: &str) -> (String, String) {
    (name.to_string(), vers.to_string())
}

#[test]
fn adding_a_dependency_keeps_locked_versions_and_the_format() {
    let published = vec![
        version("a", "1.0.0"),
        version("a", "1.1.0"),
        version("b", "1.0.0"),
    ];
    let lock = lock_with(&["a"], &[("a", "1.0.0", &[])]);
    let locked = fetch(published, "a = \"1\"\nb = \"1\"", Some(&lock));
    assert_eq!(locked.packages, [pkg("a", "1.0.0"), pkg("b", "1.0.0")]);
    assert_eq!(locked.format, 4);
}

#[test]
fn a_version_without_a_needed_feature_is_not_chosen() {
    // a 1.1.0 dropped feature `f`, which `needs-f` asks for: Cargo picks a 1.0.0.
    let mut needs = version("needs-f", "1.0.0");
    needs.deps = vec![dep("a", None, "^1", &["f"])];
    let mut a_old = version("a", "1.0.0");
    a_old.features = "{\"f\":[]}";
    let published = vec![a_old, version("a", "1.1.0"), needs];
    let locked = fetch(published, "a = \"1\"\nneeds-f = \"1\"", None);
    assert_eq!(
        locked.packages,
        [pkg("a", "1.0.0"), pkg("needs-f", "1.0.0")]
    );
}

#[test]
fn a_locked_version_without_a_newly_needed_feature_moves_up() {
    // The lock pins a 1.0.0; the new dependency needs `f`, which only a 1.1.0 has.
    let mut needs = version("needs-f", "1.0.0");
    needs.deps = vec![dep("a", None, "^1", &["f"])];
    let mut a_new = version("a", "1.1.0");
    a_new.features = "{\"f\":[]}";
    let published = vec![version("a", "1.0.0"), a_new, needs];
    let lock = lock_with(&["a"], &[("a", "1.0.0", &[])]);
    let locked = fetch(published, "a = \"1\"\nneeds-f = \"1\"", Some(&lock));
    assert_eq!(
        locked.packages,
        [pkg("a", "1.1.0"), pkg("needs-f", "1.0.0")]
    );
}

#[test]
fn a_conflict_in_one_major_leaves_the_other_major_alone() {
    // The lock pins c 1.0.0 and c 2.0.0. `d` needs c ^1.2 (a restart moves 1.x to 1.2.0)
    // and c 2 renamed: the 1.x pin must not reach 2.x, and d lists both versions.
    let mut d = version("d", "1.0.0");
    d.deps = vec![dep("c", None, "^1.2", &[]), dep("c2", Some("c"), "^2", &[])];
    let published = vec![
        version("c", "1.0.0"),
        version("c", "1.2.0"),
        version("c", "2.0.0"),
        d,
    ];
    let lock = lock_with(
        &["c 1.0.0", "c 2.0.0"],
        &[("c", "1.0.0", &[]), ("c", "2.0.0", &[])],
    );
    let locked = fetch(
        published,
        "c = \"1\"\nc2 = { package = \"c\", version = \"2\" }\nd = \"1\"",
        Some(&lock),
    );
    assert_eq!(
        locked.packages,
        [pkg("c", "1.2.0"), pkg("c", "2.0.0"), pkg("d", "1.0.0")]
    );
    let mut d_edges = locked.edges["d 1.0.0"].clone();
    d_edges.sort();
    assert_eq!(d_edges, ["c 1.2.0", "c 2.0.0"]);
}
