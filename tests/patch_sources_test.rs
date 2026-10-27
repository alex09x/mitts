/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! `[patch.crates-io]`: a registry dependency replaced by a path or git package. Without
//! a lockfile the replacement is resolved and pinned; with the lockfile Cargo would write,
//! the patched code is what gets compiled. The registry itself is never contacted.

#![cfg(unix)]

use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
        .args(args)
        .output()
        .expect("git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn write_lib(dir: &Path, name: &str, version: &str, body: &str) {
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(
        dir.join("Cargo.toml"),
        format!("[package]\nname = \"{name}\"\nversion = \"{version}\"\nedition = \"2021\"\n"),
    )
    .unwrap();
    fs::write(dir.join("src/lib.rs"), body).unwrap();
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
        .env("MITTS_SPARSE_INDEX_URL", "http://127.0.0.1:9")
        .env("MITTS_CRATES_IO_URL", "http://127.0.0.1:9")
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

#[test]
fn a_path_patch_replaces_the_registry_package_with_and_without_a_lockfile() {
    let temp = TempDir::new().unwrap();
    let cargo_home = temp.path().join("cargo_home");
    fs::create_dir_all(&cargo_home).unwrap();
    // The patched package lives next to the app and declares the same name/version
    // the app asks crates.io for.
    write_lib(
        &temp.path().join("fixed-semver"),
        "semverish",
        "1.2.0",
        "pub fn v() -> &'static str { \"patched\" }\n",
    );
    let app = temp.path().join("app");
    fs::create_dir_all(app.join("src")).unwrap();
    fs::write(
        app.join("Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nsemverish = \"1.2\"\n\n[patch.crates-io]\nsemverish = { path = \"../fixed-semver\" }\n",
    )
    .unwrap();
    fs::write(
        app.join("src/main.rs"),
        "fn main() { println!(\"{}\", semverish::v()); }\n",
    )
    .unwrap();
    let cache = temp.path().join("cache");

    // No lockfile and no registry reachable: the patch alone resolves the dependency.
    let (ok, text) = mitts(&["build"], &app, &cache, &cargo_home);
    assert!(ok, "path patch without a lockfile:\n{text}");
    let lock = fs::read_to_string(app.join("Cargo.lock")).unwrap();
    assert!(
        lock.contains("name = \"semverish\"\nversion = \"1.2.0\"\n")
            && !lock.contains("semverish\"\nversion = \"1.2.0\"\nsource"),
        "the patched package has no source, like a path package:\n{lock}"
    );
    let run = Command::new(app.join("target/mitts/debug/app"))
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "patched");

    // The lockfile written is reused as is.
    let (ok, text) = mitts(&["build"], &app, &cache, &cargo_home);
    assert!(ok && text.contains("100% cache hit"), "{text}");

    // A patch that does not satisfy the requirement is refused, as Cargo refuses it.
    fs::write(
        app.join("Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nsemverish = \"2\"\n\n[patch.crates-io]\nsemverish = { path = \"../fixed-semver\" }\n",
    )
    .unwrap();
    fs::remove_file(app.join("Cargo.lock")).unwrap();
    let (ok, text) = mitts(&["build"], &app, &temp.path().join("cache2"), &cargo_home);
    assert!(!ok && text.contains("does not satisfy"), "{text}");
}

#[test]
fn a_git_patch_is_pinned_to_its_commit_and_used_for_the_build() {
    let temp = TempDir::new().unwrap();
    let cargo_home = temp.path().join("cargo_home");
    fs::create_dir_all(&cargo_home).unwrap();
    let work = temp.path().join("work");
    write_lib(
        &work,
        "semverish",
        "1.3.1",
        "pub fn v() -> &'static str { \"from git\" }\n",
    );
    git(&work, &["init", "-q", "-b", "main"]);
    git(&work, &["add", "."]);
    git(&work, &["commit", "-q", "-m", "fix"]);
    let sha = git(&work, &["rev-parse", "HEAD"]);
    git(
        temp.path(),
        &["clone", "-q", "--bare", "work", "semverish.git"],
    );
    let url = format!("file://{}", temp.path().join("semverish.git").display());

    let app = temp.path().join("app");
    fs::create_dir_all(app.join("src")).unwrap();
    fs::write(
        app.join("Cargo.toml"),
        format!(
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nsemverish = \"1\"\n\n[patch.crates-io]\nsemverish = {{ git = \"{url}\" }}\n"
        ),
    )
    .unwrap();
    fs::write(
        app.join("src/main.rs"),
        "fn main() { println!(\"{}\", semverish::v()); }\n",
    )
    .unwrap();
    let cache = temp.path().join("cache");
    let (ok, text) = mitts(&["build"], &app, &cache, &cargo_home);
    assert!(ok, "git patch without a lockfile:\n{text}");
    let lock = fs::read_to_string(app.join("Cargo.lock")).unwrap();
    assert!(
        lock.contains(&format!("source = \"git+{url}#{sha}\"")),
        "{lock}"
    );
    let run = Command::new(app.join("target/mitts/debug/app"))
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "from git");
}
