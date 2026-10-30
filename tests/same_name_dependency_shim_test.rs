/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! rand_core 0.3 is a shim: `extern crate rand_core as core4;` re-exports rand_core 0.4, a
//! dependency with the shim's own crate name. Mitts dropped every `--extern` whose name
//! equalled the unit's own package name, so the shim's unit got no `--extern rand_core=`
//! and rustc found two candidates on the search path (petgraph's dependency graph).

use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

fn write_project(dir: &Path) {
    for sub in ["app/src", "shim03/src", "shim04/src"] {
        fs::create_dir_all(dir.join(sub)).unwrap();
    }
    fs::write(
        dir.join("app/Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
         [dependencies]\nrand_core = { path = \"../shim03\", version = \"0.3.0\" }\n",
    )
    .unwrap();
    fs::write(dir.join("app/src/lib.rs"), "pub fn v() -> u32 { rand_core::three() }\n").unwrap();
    fs::write(
        dir.join("shim03/Cargo.toml"),
        "[package]\nname = \"rand_core\"\nversion = \"0.3.0\"\nedition = \"2021\"\n\n\
         [dependencies]\nrand_core = { path = \"../shim04\", version = \"0.4.0\" }\n",
    )
    .unwrap();
    fs::write(
        dir.join("shim03/src/lib.rs"),
        "extern crate rand_core as core4;\npub fn three() -> u32 { core4::four() - 1 }\n",
    )
    .unwrap();
    fs::write(
        dir.join("shim04/Cargo.toml"),
        "[package]\nname = \"rand_core\"\nversion = \"0.4.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(dir.join("shim04/src/lib.rs"), "pub fn four() -> u32 { 4 }\n").unwrap();
}

#[test]
fn a_shim_crate_gets_its_same_named_dependency_as_extern() {
    let cache = TempDir::new().unwrap();
    let dir = TempDir::new().unwrap();
    write_project(dir.path());
    let out = Command::new(env!("CARGO_BIN_EXE_mitts"))
        .args(["check", "--manifest-path"])
        .arg(dir.path().join("app/Cargo.toml"))
        .arg("--cache-dir")
        .arg(cache.path())
        .env("CI", "true")
        .env_remove("MITTS_CACHE_DIR")
        .output()
        .expect("run mitts");
    assert!(
        out.status.success(),
        "the shim compiles with --extern rand_core= pointing at the 0.4 library:\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}
