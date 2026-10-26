/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! An artifact that embeds its checkout path is published under a per-checkout
//! `:ws_leak:` key. Once that is known, other checkouts building the same unit must not
//! queue on the shared single-flight lock: their results cannot be shared anyway, and a
//! swarm of 12 worktrees otherwise compiles the unit twelve times in a row.

use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

/// A workspace whose binary embeds its own manifest dir through a path dependency's
/// macro: the consumer's source never mentions `CARGO_MANIFEST_DIR`, so only the
/// publish-time scan can see the leak.
fn write_workspace(dir: &Path) {
    fs::create_dir_all(dir.join("app/src")).unwrap();
    fs::create_dir_all(dir.join("here/src")).unwrap();
    fs::write(
        dir.join("Cargo.toml"),
        "[workspace]\nmembers = [\"app\", \"here\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    fs::write(
        dir.join("here/Cargo.toml"),
        "[package]\nname = \"here\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(
        dir.join("here/src/lib.rs"),
        "#[macro_export]\nmacro_rules! here { () => { env!(\"CARGO_MANIFEST_DIR\") }; }\n",
    )
    .unwrap();
    fs::write(
        dir.join("app/Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nhere = { path = \"../here\" }\n",
    )
    .unwrap();
    fs::write(
        dir.join("app/src/main.rs"),
        "fn main() { println!(\"{}\", here::here!()); }\n",
    )
    .unwrap();
}

fn mitts_build(dir: &Path, cache: &Path) {
    let out = Command::new(env!("CARGO_BIN_EXE_mitts"))
        .args(["build", "--manifest-path"])
        .arg(dir.join("Cargo.toml"))
        .arg("--cache-dir")
        .arg(cache)
        .env("CI", "true")
        .env_remove("MITTS_CACHE_DIR")
        .output()
        .expect("run mitts");
    assert!(
        out.status.success(),
        "mitts build failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn a_path_embedding_artifact_leaves_a_leak_marker_and_per_checkout_locks() {
    let cache = TempDir::new().unwrap();
    let first = TempDir::new().unwrap();
    write_workspace(first.path());
    mitts_build(first.path(), cache.path());

    let markers = cache.path().join("ac").join("ws_leak");
    let count = fs::read_dir(&markers).map(|d| d.count()).unwrap_or(0);
    assert!(
        count >= 1,
        "the binary embeds its checkout path, so a leak marker must be recorded ({markers:?})"
    );
    let marker = fs::read_dir(&markers)
        .unwrap()
        .flatten()
        .next()
        .unwrap()
        .path();
    let leaked_key_digest = marker.file_name().unwrap().to_string_lossy().into_owned();
    assert_eq!(
        leaked_key_digest.len(),
        64,
        "markers are keyed by the result key digest"
    );

    // A second checkout of the same source also has to build the unit itself.
    let second = TempDir::new().unwrap();
    write_workspace(second.path());
    mitts_build(second.path(), cache.path());
    let entries: Vec<String> = fs::read_dir(cache.path().join("ac"))
        .unwrap()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| fs::read_to_string(e.path()).ok())
        .filter(|body| body.contains(":ws_leak:"))
        .collect();
    assert!(
        entries.len() >= 2,
        "each checkout publishes its own :ws_leak: entry, found {}",
        entries.len()
    );
}
