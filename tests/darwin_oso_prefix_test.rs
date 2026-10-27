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
//! stabs. Direct dependencies reach rustc through `--extern <store>/artifacts/…`, but
//! transitive ones are found through `-L dependency=<checkout>/target/…/deps`, so a
//! binary with a two-hop dependency chain named its checkout and was keyed per checkout.
//! With `-oso_prefix` those paths are relative to the workspace: no checkout path in the
//! binary, and a second checkout restores it instead of linking again. Workspace rlibs
//! leave no OSO entry of their own (their debug info is remapped away), so the transitive
//! hop is a crates.io crate: the test needs the network once.

#![cfg(target_os = "macos")]

use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

/// `osoapp` (bin) → `mid` (lib) → `semver` (crates.io): `semver` is transitive for the
/// binary, so rustc finds its rlib through the checkout's deps directory.
fn write_project(dir: &Path) {
    for sub in ["src", "mid/src"] {
        fs::create_dir_all(dir.join(sub)).unwrap();
    }
    fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname = \"osoapp\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nmid = { path = \"mid\" }\n\n[workspace]\nmembers = [\"mid\"]\n",
    )
    .unwrap();
    fs::write(
        dir.join("mid/Cargo.toml"),
        "[package]\nname = \"mid\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nsemver = \"=1.0.23\"\n",
    )
    .unwrap();
    fs::write(
        dir.join("mid/src/lib.rs"),
        "#[inline(never)]\npub fn mid_val(x: u64) -> u64 { semver::Version::parse(\"6.0.0\").unwrap().major * x + 1 }\n",
    )
    .unwrap();
    fs::write(
        dir.join("src/main.rs"),
        "fn main() { println!(\"{}\", mid::mid_val(7) - 1); }\n",
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
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "42");

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
    // The transitive rlib was linked from the checkout's deps directory and is recorded
    // relative to the workspace: this is the entry that used to be absolute.
    assert!(
        oso.iter()
            .any(|l| l.contains(" OSO target/mitts/debug/deps/libsemver")),
        "the transitive semver rlib is recorded relative to the workspace:\n{}",
        oso.join("\n")
    );
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
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "42");
}
