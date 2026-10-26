/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A `CARGO_MANIFEST_DIR` read keys only the targets that can compile it on the
//! absolute checkout path. `MITTS_LOG_KEYS=1` names every unit keyed that way.

use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

const READER: &str = "pub fn here() -> &'static str { env!(\"CARGO_MANIFEST_DIR\") }\n";

fn write_package(dir: &Path, lib_reads: bool) {
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::create_dir_all(dir.join("tests")).unwrap();
    fs::create_dir_all(dir.join("examples")).unwrap();
    fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname = \"scoped\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(
        dir.join("src/lib.rs"),
        if lib_reads {
            READER.to_string()
        } else {
            "pub fn here() -> &'static str { \"nowhere\" }\n".to_string()
        },
    )
    .unwrap();
    fs::write(
        dir.join("examples/listing.rs"),
        format!("{READER}fn main() {{ here(); }}\n"),
    )
    .unwrap();
    fs::write(
        dir.join("tests/reads.rs"),
        "#[test]\nfn reads() { assert!(!env!(\"CARGO_MANIFEST_DIR\").is_empty()); }\n",
    )
    .unwrap();
    fs::write(
        dir.join("tests/plain.rs"),
        "#[test]\nfn plain() { assert_eq!(scoped::here().len(), scoped::here().len()); }\n",
    )
    .unwrap();
}

/// Units `mitts test --no-run` keyed on the absolute manifest dir.
fn locked_units(dir: &Path, cache: &Path) -> Vec<String> {
    let out = Command::new(env!("CARGO_BIN_EXE_mitts"))
        .args(["test", "--tests", "--no-run", "--manifest-path"])
        .arg(dir.join("Cargo.toml"))
        .arg("--cache-dir")
        .arg(cache)
        .env("CI", "true")
        .env("MITTS_LOG_KEYS", "1")
        .env_remove("MITTS_CACHE_DIR")
        .output()
        .expect("run mitts");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "mitts test --no-run failed:\n{stderr}"
    );
    let mut units: Vec<String> = stderr
        .lines()
        .filter(|l| l.contains("keyed on the absolute manifest dir"))
        .filter_map(|l| l.split_whitespace().nth(1).map(str::to_string))
        .collect();
    units.sort();
    units.dedup();
    units
}

#[test]
fn only_the_targets_that_read_the_manifest_dir_are_path_locked() {
    let cache = TempDir::new().unwrap();
    let temp = TempDir::new().unwrap();
    let dir = temp.path().join("scoped");

    write_package(&dir, false);
    let locked = locked_units(&dir, cache.path());
    assert_eq!(
        locked,
        vec!["scoped@0.1.0::test:reads".to_string()],
        "an example and one integration test read the dir; only that test is locked"
    );

    write_package(&dir, true);
    let locked = locked_units(&dir, cache.path());
    assert!(
        locked.iter().any(|u| u.ends_with("::lib:scoped")),
        "a reader in src/ locks the library: {locked:?}"
    );
    assert!(locked.iter().any(|u| u == "scoped@0.1.0::test:reads"));
    assert!(
        !locked.iter().any(|u| u == "scoped@0.1.0::test:plain"),
        "an integration test that does not read the dir stays shareable: {locked:?}"
    );
}
