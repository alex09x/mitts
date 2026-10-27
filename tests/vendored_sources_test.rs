/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! `cargo vendor` directories: `[source.crates-io] replace-with = "vendored-sources"` and
//! `[source.vendored-sources] directory = "vendor"` make a build work with no network at
//! all, for crates.io packages and for git packages alike; a vendored file that was edited
//! is refused by its `.cargo-checksum.json`.

#![cfg(unix)]

use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

/// Writes `vendor/<dir>` for one package with a valid `.cargo-checksum.json`.
fn vendor_package(
    vendor: &Path,
    dir: &str,
    name: &str,
    version: &str,
    body: &str,
    package_sum: Option<&str>,
) {
    let root = vendor.join(dir);
    fs::create_dir_all(root.join("src")).unwrap();
    let manifest =
        format!("[package]\nname = \"{name}\"\nversion = \"{version}\"\nedition = \"2021\"\n");
    fs::write(root.join("Cargo.toml"), &manifest).unwrap();
    fs::write(root.join("src/lib.rs"), body).unwrap();
    let sum = |t: &str| format!("{:x}", Sha256::digest(t.as_bytes()));
    let package = package_sum
        .map(|p| format!("\"{p}\""))
        .unwrap_or_else(|| "null".to_string());
    fs::write(
        root.join(".cargo-checksum.json"),
        format!(
            "{{\"files\":{{\"Cargo.toml\":\"{}\",\"src/lib.rs\":\"{}\"}},\"package\":{package}}}",
            sum(&manifest),
            sum(body)
        ),
    )
    .unwrap();
}

fn mitts(args: &[&str], dir: &Path, cache: &Path, cargo_home: &Path) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_mitts"))
        .args(args)
        .arg("--manifest-path")
        .arg(dir.join("Cargo.toml"))
        .arg("--cache-dir")
        .arg(cache)
        .env("CI", "true")
        .env("CARGO_HOME", cargo_home)
        // No network: crates.io at a closed port, git through a binary that always fails.
        .env("MITTS_SPARSE_INDEX_URL", "http://127.0.0.1:9")
        .env("MITTS_CRATES_IO_URL", "http://127.0.0.1:9")
        .env("MITTS_GIT", "/bin/false")
        .env_remove("MITTS_CACHE_DIR")
        .output()
        .expect("run mitts");
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success(), text)
}

const PKG_SUM: &str = "1111111111111111111111111111111111111111111111111111111111111111";

#[test]
fn vendored_crates_io_packages_build_without_network_with_and_without_a_lockfile() {
    let temp = TempDir::new().unwrap();
    let cargo_home = temp.path().join("cargo_home");
    fs::create_dir_all(&cargo_home).unwrap();
    let app = temp.path().join("app");
    fs::create_dir_all(app.join("src")).unwrap();
    fs::create_dir_all(app.join(".cargo")).unwrap();
    // `cargo vendor` names a directory after the package, or `<name>-<version>` when two
    // versions are vendored: both layouts must be found.
    vendor_package(
        &app.join("vendor"),
        "vend-one",
        "vend-one",
        "0.2.0",
        "pub fn v() -> u32 { 2 }\n",
        Some(PKG_SUM),
    );
    vendor_package(
        &app.join("vendor"),
        "vend-two-1.0.0",
        "vend-two",
        "1.0.0",
        "pub fn w() -> u32 { 40 }\n",
        Some(PKG_SUM),
    );
    fs::write(
        app.join(".cargo/config.toml"),
        "[source.crates-io]\nreplace-with = \"vendored-sources\"\n\n[source.vendored-sources]\ndirectory = \"vendor\"\n",
    )
    .unwrap();
    fs::write(
        app.join("Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nvend-one = \"0.2\"\nvend-two = \"1\"\n",
    )
    .unwrap();
    fs::write(
        app.join("src/main.rs"),
        "fn main() { println!(\"{}\", vend_one::v() + vend_two::w()); }\n",
    )
    .unwrap();
    let cache = temp.path().join("cache");

    // No lockfile: versions come from the vendor directory, not from an index.
    let (ok, text) = mitts(&["build"], &app, &cache, &cargo_home);
    assert!(ok, "vendored build without a lockfile:\n{text}");
    let lock = fs::read_to_string(app.join("Cargo.lock")).unwrap();
    assert!(
        lock.contains("source = \"registry+https://github.com/rust-lang/crates.io-index\"")
            && lock.contains(&format!("checksum = \"{PKG_SUM}\"")),
        "the lockfile looks exactly as with crates.io:\n{lock}"
    );
    let run = Command::new(app.join("target/mitts/debug/app"))
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "42");

    // With the lockfile, a fresh cache still needs no network.
    let cache2 = temp.path().join("cache2");
    let (ok, text) = mitts(&["build"], &app, &cache2, &cargo_home);
    assert!(ok, "{text}");
    assert!(
        text.contains("100% cache hit") || text.contains("Compiled"),
        "{text}"
    );

    // An edited vendored file is refused.
    fs::write(
        app.join("vendor/vend-one/src/lib.rs"),
        "pub fn v() -> u32 { 3 }\n",
    )
    .unwrap();
    let cache3 = temp.path().join("cache3");
    let (ok, text) = mitts(&["build"], &app, &cache3, &cargo_home);
    assert!(!ok && text.contains("was modified"), "{text}");
}

#[test]
fn a_vendored_git_package_needs_no_git_access() {
    let temp = TempDir::new().unwrap();
    let cargo_home = temp.path().join("cargo_home");
    fs::create_dir_all(&cargo_home).unwrap();
    let app = temp.path().join("app");
    fs::create_dir_all(app.join("src")).unwrap();
    fs::create_dir_all(app.join(".cargo")).unwrap();
    let url = "https://git.example.invalid/org/gitpkg";
    let sha = "0123456789abcdef0123456789abcdef01234567";
    vendor_package(
        &app.join("vendor"),
        "gitpkg",
        "gitpkg",
        "0.3.0",
        "pub fn g() -> u32 { 7 }\n",
        None,
    );
    fs::write(
        app.join(".cargo/config.toml"),
        format!(
            "[source.\"git+{url}?tag=v0.3.0\"]\ngit = \"{url}\"\ntag = \"v0.3.0\"\nreplace-with = \"vendored-sources\"\n\n[source.vendored-sources]\ndirectory = \"vendor\"\n"
        ),
    )
    .unwrap();
    fs::write(
        app.join("Cargo.toml"),
        format!("[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\ngitpkg = {{ git = \"{url}\", tag = \"v0.3.0\" }}\n"),
    )
    .unwrap();
    fs::write(
        app.join("src/main.rs"),
        "fn main() { println!(\"{}\", gitpkg::g()); }\n",
    )
    .unwrap();
    fs::write(
        app.join("Cargo.lock"),
        format!(
            "version = 3\n\n[[package]]\nname = \"app\"\nversion = \"0.1.0\"\ndependencies = [\n \"gitpkg\",\n]\n\n[[package]]\nname = \"gitpkg\"\nversion = \"0.3.0\"\nsource = \"git+{url}?tag=v0.3.0#{sha}\"\n"
        ),
    )
    .unwrap();
    let (ok, text) = mitts(&["build"], &app, &temp.path().join("cache"), &cargo_home);
    assert!(ok, "vendored git package, git disabled:\n{text}");
    let run = Command::new(app.join("target/mitts/debug/app"))
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "7");
}
