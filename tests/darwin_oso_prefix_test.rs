/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! On macOS the linker records each linked object's absolute path in the output's N_OSO
//! stabs; the dependency rlibs Mitts restores under the checkout's `target/` therefore put
//! the checkout path into every binary, which the leak scanner (rightly) keyed per
//! checkout. With `-oso_prefix` those paths are relative to the workspace: no checkout
//! path in the binary, and a second checkout restores it instead of linking again.
//!
//! Needs the network once: the dependency comes from crates.io.

#![cfg(target_os = "macos")]

use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

fn write_project(dir: &Path) {
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname = \"osoapp\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nsemver = \"=1.0.23\"\n",
    )
    .unwrap();
    fs::write(
        dir.join("src/main.rs"),
        "fn main() {\n    let v = semver::Version::parse(\"1.2.3\").unwrap();\n    println!(\"{}.{}\", v.major, v.minor + v.patch);\n}\n",
    )
    .unwrap();
}

fn mitts_build(dir: &Path, cache: &Path) -> String {
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
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn a_linked_binary_names_no_checkout_path_and_is_shared_across_checkouts() {
    let cache = TempDir::new().unwrap();
    let first = TempDir::new().unwrap();
    write_project(first.path());
    mitts_build(first.path(), cache.path());

    let bin = first.path().join("target/mitts/debug/osoapp");
    let run = Command::new(&bin).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "1.5");

    // The dependency's objects are recorded relative to the workspace, never absolutely.
    let nm = Command::new("nm").args(["-ap"]).arg(&bin).output().unwrap();
    let stabs = String::from_utf8_lossy(&nm.stdout);
    let oso: Vec<&str> = stabs.lines().filter(|l| l.contains(" OSO ")).collect();
    let root = first.path().canonicalize().unwrap();
    for line in &oso {
        assert!(
            !line.contains(root.to_str().unwrap())
                && !line.contains(first.path().to_str().unwrap()),
            "OSO entry still names the checkout: {line}"
        );
    }
    // Where the rlib was linked from depends on the restore mode (the store's artifacts
    // directory or the checkout's target/); either way no OSO entry may name the checkout.
    assert!(!oso.is_empty(), "a dev binary carries OSO stabs:\n{stabs}");
    let bytes = fs::read(&bin).unwrap();
    let needle = root.to_str().unwrap().as_bytes();
    assert!(
        !bytes.windows(needle.len()).any(|w| w == needle),
        "the checkout path appears nowhere in the binary"
    );

    // No leak marker, and a second checkout of the same source is a full cache hit.
    assert!(
        fs::read_dir(cache.path().join("ac/ws_leak"))
            .map(|d| d.count() == 0)
            .unwrap_or(true),
        "no :ws_leak: marker for a binary without checkout paths"
    );
    let second = TempDir::new().unwrap();
    write_project(second.path());
    let text = mitts_build(second.path(), cache.path());
    assert!(
        text.contains("100% cache hit"),
        "the second checkout restores the binary instead of linking it:\n{text}"
    );
    let run = Command::new(second.path().join("target/mitts/debug/osoapp"))
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "1.5");
}
