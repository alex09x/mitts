/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! The units of one package (library, its unit tests, integration tests) share one
//! build-script run on a cold build instead of each executing the script in parallel.

use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

fn write_package(dir: &Path) {
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::create_dir_all(dir.join("tests")).unwrap();
    fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname = \"counted\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    // Every run appends one line next to the package: the unit count is the run count.
    fs::write(
        dir.join("build.rs"),
        r#"fn main() {
    let log = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("runs.log");
    let mut f = std::fs::OpenOptions::new().append(true).create(true).open(log).unwrap();
    use std::io::Write;
    writeln!(f, "run").unwrap();
    println!("cargo:rustc-env=COUNTED=yes");
}
"#,
    )
    .unwrap();
    fs::write(
        dir.join("src/lib.rs"),
        "pub fn tag() -> &'static str { env!(\"COUNTED\") }\n#[cfg(test)]\nmod t { #[test] fn unit() { assert_eq!(super::tag(), \"yes\"); } }\n",
    )
    .unwrap();
    fs::write(
        dir.join("tests/it.rs"),
        "#[test]\nfn it() { assert_eq!(counted::tag(), \"yes\"); }\n",
    )
    .unwrap();
}

#[test]
fn a_cold_build_runs_the_package_build_script_once_for_all_its_units() {
    let cache = TempDir::new().unwrap();
    let temp = TempDir::new().unwrap();
    let dir = temp.path().join("counted");
    write_package(&dir);
    let out = Command::new(env!("CARGO_BIN_EXE_mitts"))
        .args(["test", "--tests", "--no-run", "--manifest-path"])
        .arg(dir.join("Cargo.toml"))
        .arg("--cache-dir")
        .arg(cache.path())
        .env("CI", "true")
        .env_remove("MITTS_CACHE_DIR")
        .output()
        .expect("run mitts");
    assert!(
        out.status.success(),
        "mitts test --no-run failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let runs = fs::read_to_string(dir.join("runs.log")).unwrap_or_default();
    assert_eq!(
        runs.lines().count(),
        1,
        "lib, lib unit test and integration test must share one build-script run:\n{runs}"
    );
}
