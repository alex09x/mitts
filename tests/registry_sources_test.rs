/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Registries other than crates.io, as Cargo configures them: `[registries.<name>]` with a
//! sparse index and a token, and `[source.crates-io] replace-with` pointing at a mirror.
//! A tiny HTTP server plays the registry; the real crates.io is pointed at a closed port.

#![cfg(unix)]

use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tempfile::TempDir;

/// A `.crate` archive for a one-function library, with its sha256.
fn crate_archive(name: &str, version: &str, body: &str) -> (Vec<u8>, String) {
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
            &format!("[package]\nname = \"{name}\"\nversion = \"{version}\"\nedition = \"2021\"\n"),
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

/// A registry serving `alt-one 0.1.0`: sparse index under `/index`, crates under `/dl`.
fn registry(token: Option<&'static str>) -> (String, Arc<AtomicUsize>, String) {
    let (bytes, sum) = crate_archive("alt-one", "0.1.0", "pub fn one() -> u32 { 1 }\n");
    let mut routes = HashMap::new();
    routes.insert(
        format!("/index/{}", index_path("alt-one")),
        format!(
            "{{\"name\":\"alt-one\",\"vers\":\"0.1.0\",\"deps\":[],\"cksum\":\"{sum}\",\"features\":{{}},\"yanked\":false}}\n"
        )
        .into_bytes(),
    );
    routes.insert("/dl/alt-one/0.1.0/download".to_string(), bytes);
    let placeholder = "__BASE__";
    routes.insert(
        "/index/config.json".to_string(),
        format!(
            "{{\"dl\":\"{placeholder}/dl\",\"api\":\"{placeholder}\",\"auth-required\":{}}}",
            token.is_some()
        )
        .into_bytes(),
    );
    // The base URL is only known after binding: patch it into config.json. The listener that
    // picked the port is the one that serves: dropping it and binding the port again let a
    // parallel test take it in between (AddrInUse).
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let base = format!("http://127.0.0.1:{port}");
    let cfg = routes.get_mut("/index/config.json").unwrap();
    *cfg = String::from_utf8(cfg.clone())
        .unwrap()
        .replace(placeholder, &base)
        .into_bytes();
    let (url, hits) = serve_on(routes, token, listener);
    assert_eq!(url, base);
    (url, hits, sum)
}

/// Serves `routes` (path → body) over HTTP on `listener`; with `token`, every request must
/// carry it in `Authorization`.
fn serve_on(
    routes: HashMap<String, Vec<u8>>,
    token: Option<&'static str>,
    listener: TcpListener,
) -> (String, Arc<AtomicUsize>) {
    let addr = listener.local_addr().unwrap();
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut stream = stream;
            let mut buf = [0u8; 8192];
            let n = stream.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            let path = req.split_whitespace().nth(1).unwrap_or("/").to_string();
            counter.fetch_add(1, Ordering::SeqCst);
            let authorized = token.is_none_or(|t| {
                req.lines()
                    .any(|l| l.to_ascii_lowercase().starts_with("authorization:") && l.contains(t))
            });
            let (status, body) = if !authorized {
                ("401 Unauthorized", b"token required".to_vec())
            } else if let Some(b) = routes.get(&path) {
                ("200 OK", b.clone())
            } else {
                ("404 Not Found", Vec::new())
            };
            let _ = write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(&body);
        }
    });
    (format!("http://{addr}"), hits)
}

fn write_project(dir: &Path, cargo_config: &str, dep_line: &str) {
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::create_dir_all(dir.join(".cargo")).unwrap();
    fs::write(dir.join(".cargo/config.toml"), cargo_config).unwrap();
    fs::write(
        dir.join("Cargo.toml"),
        format!("[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n{dep_line}\n"),
    )
    .unwrap();
    fs::write(
        dir.join("src/main.rs"),
        "fn main() { println!(\"{}\", alt_one::one()); }\n",
    )
    .unwrap();
}

fn mitts(
    args: &[&str],
    dir: &Path,
    cache: &Path,
    cargo_home: &Path,
    env: &[(&str, &str)],
) -> (bool, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mitts"));
    cmd.args(args)
        .arg("--manifest-path")
        .arg(dir.join("Cargo.toml"))
        .arg("--cache-dir")
        .arg(cache)
        .env("CI", "true")
        .env("CARGO_HOME", cargo_home)
        // The real crates.io must never be reached: closed local port.
        .env("MITTS_SPARSE_INDEX_URL", "http://127.0.0.1:9")
        .env("MITTS_CRATES_IO_URL", "http://127.0.0.1:9")
        .env_remove("MITTS_CACHE_DIR");
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd.output().expect("run mitts");
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success(), text)
}

#[test]
fn an_alternative_sparse_registry_with_a_token_resolves_fetches_and_builds() {
    let (url, hits, sum) = registry(Some("s3cret"));
    let temp = TempDir::new().unwrap();
    let cargo_home = temp.path().join("cargo_home");
    fs::create_dir_all(&cargo_home).unwrap();
    let app = temp.path().join("app");
    write_project(
        &app,
        &format!("[registries]\nalt = {{ index = \"sparse+{url}/index/\" }}\n"),
        "alt-one = { version = \"0.1\", registry = \"alt\" }",
    );
    let cache = temp.path().join("cache");

    // Without a token the registry refuses and Mitts says what is missing.
    let (ok, text) = mitts(&["build"], &app, &cache, &cargo_home, &[]);
    assert!(!ok && text.contains("requires authentication"), "{text}");
    assert!(!app.join("Cargo.lock").exists());

    // The token comes from the environment, as `CARGO_REGISTRIES_ALT_TOKEN` does for Cargo.
    let (ok, text) = mitts(
        &["build"],
        &app,
        &cache,
        &cargo_home,
        &[("CARGO_REGISTRIES_ALT_TOKEN", "s3cret")],
    );
    assert!(ok, "build through the alternative registry:\n{text}");
    let lock = fs::read_to_string(app.join("Cargo.lock")).unwrap();
    assert!(
        lock.contains(&format!("source = \"sparse+{url}/index/\"")),
        "{lock}"
    );
    assert!(lock.contains(&format!("checksum = \"{sum}\"")), "{lock}");
    let run = Command::new(app.join("target/debug/app")).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "1");
    assert!(
        hits.load(Ordering::SeqCst) >= 3,
        "config.json, index entry and crate were served"
    );
    let served = hits.load(Ordering::SeqCst);

    // Warm: nothing is fetched again, and the package sits in its own sub-store.
    let (ok, text) = mitts(
        &["build"],
        &app,
        &cache,
        &cargo_home,
        &[("CARGO_REGISTRIES_ALT_TOKEN", "s3cret")],
    );
    assert!(ok && text.contains("100% cache hit"), "{text}");
    assert_eq!(
        hits.load(Ordering::SeqCst),
        served,
        "no registry traffic on a warm build"
    );
    let sub_stores: Vec<_> = fs::read_dir(cache.join("sources"))
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("src-"))
        .collect();
    assert_eq!(
        sub_stores.len(),
        1,
        "one sub-store for the alternative registry"
    );
    assert!(sub_stores[0]
        .path()
        .join("alt-one-0.1.0/.mitts-ok")
        .is_file());

    // credentials.toml works too.
    fs::write(
        cargo_home.join("credentials.toml"),
        "[registries.alt]\ntoken = \"s3cret\"\n",
    )
    .unwrap();
    let cache2 = temp.path().join("cache2");
    let (ok, text) = mitts(&["fetch"], &app, &cache2, &cargo_home, &[]);
    assert!(ok, "token from credentials.toml:\n{text}");
}

#[test]
fn a_crates_io_mirror_keeps_the_crates_io_source_in_the_lockfile() {
    let (url, hits, sum) = registry(None);
    let temp = TempDir::new().unwrap();
    let cargo_home = temp.path().join("cargo_home");
    fs::create_dir_all(&cargo_home).unwrap();
    let app = temp.path().join("app");
    write_project(
        &app,
        &format!(
            "[source.crates-io]\nreplace-with = \"mirror\"\n[source.mirror]\nregistry = \"sparse+{url}/index/\"\n"
        ),
        "alt-one = \"0.1\"",
    );
    let cache = temp.path().join("cache");
    let (ok, text) = mitts(&["build"], &app, &cache, &cargo_home, &[]);
    assert!(ok, "build through the mirror:\n{text}");
    let lock = fs::read_to_string(app.join("Cargo.lock")).unwrap();
    assert!(
        lock.contains("source = \"registry+https://github.com/rust-lang/crates.io-index\""),
        "the lockfile names crates.io, not the mirror:\n{lock}"
    );
    assert!(lock.contains(&format!("checksum = \"{sum}\"")));
    assert!(hits.load(Ordering::SeqCst) >= 3);
    let run = Command::new(app.join("target/debug/app")).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "1");
    // crates.io packages share the flat store, mirror or not.
    assert!(cache.join("sources/alt-one-0.1.0/.mitts-ok").is_file());
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A registry whose index is a git repository: Mitts clones it, reads the entries from the
/// checkout and downloads the `.crate` from its `config.json` `dl`, as Cargo does.
#[test]
fn a_git_index_registry_resolves_fetches_and_builds() {
    let (bytes, sum) = crate_archive("alt-one", "0.1.0", "pub fn one() -> u32 { 1 }\n");
    let mut routes = HashMap::new();
    routes.insert("/dl/alt-one/0.1.0/download".to_string(), bytes);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let (base, hits) = serve_on(routes, None, listener);

    let temp = TempDir::new().unwrap();
    let index = temp.path().join("index-repo");
    let entry = index.join(index_path("alt-one"));
    fs::create_dir_all(entry.parent().unwrap()).unwrap();
    fs::write(
        &entry,
        format!(
            "{{\"name\":\"alt-one\",\"vers\":\"0.1.0\",\"deps\":[],\"cksum\":\"{sum}\",\"features\":{{}},\"yanked\":false}}\n"
        ),
    )
    .unwrap();
    fs::write(
        index.join("config.json"),
        format!("{{\"dl\":\"{base}/dl\"}}"),
    )
    .unwrap();
    git(&index, &["init", "-q"]);
    git(&index, &["add", "."]);
    git(&index, &["commit", "-q", "-m", "index"]);
    let index_url = format!("file://{}", index.display());

    let cargo_home = temp.path().join("cargo_home");
    fs::create_dir_all(&cargo_home).unwrap();
    let app = temp.path().join("app");
    write_project(
        &app,
        &format!("[registries]\nold = {{ index = \"{index_url}\" }}\n"),
        "alt-one = { version = \"0.1\", registry = \"old\" }",
    );
    let cache = temp.path().join("cache");
    let (ok, text) = mitts(&["build"], &app, &cache, &cargo_home, &[]);
    assert!(ok, "build through the git index:\n{text}");
    let lock = fs::read_to_string(app.join("Cargo.lock")).unwrap();
    assert!(
        lock.contains(&format!("source = \"registry+{index_url}\"")),
        "{lock}"
    );
    assert!(lock.contains(&format!("checksum = \"{sum}\"")), "{lock}");
    let run = Command::new(app.join("target/debug/app")).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "1");
    assert_eq!(
        hits.load(Ordering::SeqCst),
        1,
        "only the .crate goes over HTTP"
    );

    // Offline, a fresh project resolves from the cached checkout and source.
    let app2 = temp.path().join("app2");
    write_project(
        &app2,
        &format!("[registries]\nold = {{ index = \"{index_url}\" }}\n"),
        "alt-one = { version = \"0.1\", registry = \"old\" }",
    );
    let (ok, text) = mitts(&["build", "--offline"], &app2, &cache, &cargo_home, &[]);
    assert!(ok, "offline from the cached index checkout:\n{text}");
}

/// `[source] local-registry`: the index under `index/` and the `.crate` files beside it,
/// usable offline.
#[test]
fn a_local_registry_replaces_crates_io() {
    let (bytes, sum) = crate_archive("alt-one", "0.1.0", "pub fn one() -> u32 { 1 }\n");
    let temp = TempDir::new().unwrap();
    let registry = temp.path().join("local-registry");
    let entry = registry.join("index").join(index_path("alt-one"));
    fs::create_dir_all(entry.parent().unwrap()).unwrap();
    fs::write(
        &entry,
        format!(
            "{{\"name\":\"alt-one\",\"vers\":\"0.1.0\",\"deps\":[],\"cksum\":\"{sum}\",\"features\":{{}},\"yanked\":false}}\n"
        ),
    )
    .unwrap();
    fs::write(registry.join("alt-one-0.1.0.crate"), bytes).unwrap();

    let cargo_home = temp.path().join("cargo_home");
    fs::create_dir_all(&cargo_home).unwrap();
    let app = temp.path().join("app");
    write_project(
        &app,
        &format!(
            "[source.crates-io]\nreplace-with = \"local\"\n[source.local]\nlocal-registry = \"{}\"\n",
            registry.display()
        ),
        "alt-one = \"0.1\"",
    );
    let (ok, text) = mitts(
        &["build", "--offline"],
        &app,
        &temp.path().join("cache"),
        &cargo_home,
        &[],
    );
    assert!(ok, "build from the local registry:\n{text}");
    let lock = fs::read_to_string(app.join("Cargo.lock")).unwrap();
    assert!(
        lock.contains("source = \"registry+https://github.com/rust-lang/crates.io-index\""),
        "the lockfile names crates.io:\n{lock}"
    );
    assert!(lock.contains(&format!("checksum = \"{sum}\"")), "{lock}");
    let run = Command::new(app.join("target/debug/app")).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "1");
}

/// `fetch --target` downloads what a build for that target uses, as Cargo does: a
/// dependency under `cfg(windows)` is fetched for a Windows target only; without
/// `--target` every package in Cargo.lock is fetched.
#[test]
fn fetch_for_a_target_skips_other_platforms_dependencies() {
    let (one, one_sum) = crate_archive("alt-one", "0.1.0", "pub fn one() -> u32 { 1 }\n");
    let (two, two_sum) = crate_archive("alt-two", "0.1.0", "pub fn two() -> u32 { 2 }\n");
    let mut routes = HashMap::new();
    for (name, sum) in [("alt-one", &one_sum), ("alt-two", &two_sum)] {
        routes.insert(
            format!("/index/{}", index_path(name)),
            format!(
                "{{\"name\":\"{name}\",\"vers\":\"0.1.0\",\"deps\":[],\"cksum\":\"{sum}\",\"features\":{{}},\"yanked\":false}}\n"
            )
            .into_bytes(),
        );
    }
    routes.insert("/dl/alt-one/0.1.0/download".to_string(), one);
    routes.insert("/dl/alt-two/0.1.0/download".to_string(), two);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    routes.insert(
        "/index/config.json".to_string(),
        format!("{{\"dl\":\"{base}/dl\"}}").into_bytes(),
    );
    let (url, _) = serve_on(routes, None, listener);

    let temp = TempDir::new().unwrap();
    let cargo_home = temp.path().join("cargo_home");
    fs::create_dir_all(&cargo_home).unwrap();
    let app = temp.path().join("app");
    write_project(
        &app,
        &format!(
            "[source.crates-io]\nreplace-with = \"mirror\"\n[source.mirror]\nregistry = \"sparse+{url}/index/\"\n"
        ),
        "alt-one = \"0.1\"\n\n[target.'cfg(windows)'.dependencies]\nalt-two = \"0.1\"",
    );
    let stored = |cache: &Path, name: &str| {
        cache
            .join(format!("sources/{name}-0.1.0/.mitts-ok"))
            .is_file()
    };

    let linux = temp.path().join("cache-linux");
    let (ok, text) = mitts(
        &["fetch", "--target", "x86_64-unknown-linux-gnu"],
        &app,
        &linux,
        &cargo_home,
        &[],
    );
    assert!(ok, "{text}");
    assert!(stored(&linux, "alt-one"), "{text}");
    assert!(
        !stored(&linux, "alt-two"),
        "a Windows-only dependency: {text}"
    );
    let lock = fs::read_to_string(app.join("Cargo.lock")).unwrap();
    assert!(
        lock.contains("alt-two"),
        "the lockfile keeps every platform"
    );

    let (ok, text) = mitts(
        &["fetch", "--target", "x86_64-pc-windows-msvc"],
        &app,
        &linux,
        &cargo_home,
        &[],
    );
    assert!(ok && stored(&linux, "alt-two"), "{text}");

    let all = temp.path().join("cache-all");
    let (ok, text) = mitts(&["fetch"], &app, &all, &cargo_home, &[]);
    assert!(
        ok && stored(&all, "alt-one") && stored(&all, "alt-two"),
        "{text}"
    );
}
