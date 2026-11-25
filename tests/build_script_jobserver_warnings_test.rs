/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Two things a build script gets from Cargo and did not get from Mitts:
//! - the build's job pool in `CARGO_MAKEFLAGS`, so make, ninja or cc-rs inside the script
//!   share the `-j` limit with the rest of the build;
//! - its `cargo:warning=` lines shown by default as `warning: <pkg>@<version>: <text>`,
//!   again when the script's result comes from the cache.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use tempfile::TempDir;

const CLEARED: [&str; 8] = [
    "CARGO_TARGET_DIR",
    "CARGO_BUILD_TARGET_DIR",
    "CARGO_BUILD_TARGET",
    "RUSTC_WRAPPER",
    "RUSTC_WORKSPACE_WRAPPER",
    "CARGO_BUILD_RUSTC_WRAPPER",
    "CARGO_MAKEFLAGS",
    "MAKEFLAGS",
];

fn project(root: &Path) {
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"jp\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    // A library and a binary: two units share the one script result.
    fs::write(root.join("src/lib.rs"), "pub fn f() {}\n").unwrap();
    fs::write(root.join("src/main.rs"), "fn main() { jp::f() }\n").unwrap();
    fs::write(
        root.join("build.rs"),
        "fn main() {\n    \
         let flags = std::env::var(\"CARGO_MAKEFLAGS\").unwrap_or_default();\n    \
         println!(\"cargo:warning=makeflags=[{flags}]\");\n    \
         println!(\"cargo::warning=second line\");\n\
         }\n",
    )
    .unwrap();
}

fn mitts(root: &Path, target: &str, args: &[&str]) -> Output {
    mitts_cache(root, &root.join("cache"), target, args)
}

fn mitts_cache(root: &Path, cache: &Path, target: &str, args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mitts"));
    cmd.args(args)
        .arg("--cache-dir")
        .arg(cache)
        .arg("--target-dir")
        .arg(root.join(target))
        .current_dir(root)
        .env("CARGO_HOME", root.join("cargo-home"))
        .env_remove("MITTS_CACHE_DIR")
        .env_remove("MITTS_FAST_LOCK");
    for name in CLEARED {
        cmd.env_remove(name);
    }
    cmd.output().expect("run Mitts")
}

/// A build with the fast build lock on, as with the default cache root.
fn mitts_fast_lock(root: &Path, args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mitts"));
    cmd.args(args)
        .arg("--cache-dir")
        .arg(root.join("cache"))
        .arg("--target-dir")
        .arg(root.join("target"))
        .current_dir(root)
        .env("CARGO_HOME", root.join("cargo-home"))
        .env_remove("MITTS_CACHE_DIR")
        .env("MITTS_FAST_LOCK", "1");
    for name in CLEARED {
        cmd.env_remove(name);
    }
    cmd.output().expect("run Mitts")
}

/// The text of `warning: jp@0.1.0: <text>` lines in `log`.
fn warnings(log: &str) -> Vec<String> {
    log.lines()
        .filter_map(|l| l.trim().strip_prefix("warning: jp@0.1.0: "))
        .map(str::to_string)
        .collect()
}

#[test]
fn a_build_script_gets_the_job_pool_and_its_warnings_are_shown() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("proj");
    project(&root);

    let cold = mitts(&root, "target-a", &["build", "-j", "3"]);
    let log = String::from_utf8_lossy(&cold.stderr).to_string();
    assert!(cold.status.success(), "{log}");
    let shown = warnings(&log);
    assert_eq!(shown.len(), 2, "both warnings, once each: {log}");
    assert!(
        shown[0].starts_with("makeflags=[-j --jobserver-")
            && shown[0].contains("--jobserver-auth="),
        "the script sees the job pool like under Cargo: {log}"
    );
    assert_eq!(shown[1], "second line");

    // Another target directory restores the script's result from the cache: Cargo shows
    // a fresh script's warnings again, so does Mitts.
    let warm = mitts(&root, "target-b", &["build", "-j", "3"]);
    let log = String::from_utf8_lossy(&warm.stderr).to_string();
    assert!(warm.status.success(), "{log}");
    assert_eq!(warnings(&log).len(), 2, "replayed after a hit: {log}");
}

#[test]
fn makeflags_name_the_pool_in_cargo_form() {
    use mitts::adapter::rust::build_script_env::cargo_makeflags;
    assert_eq!(
        cargo_makeflags("fifo:/tmp/pool.fifo"),
        "-j --jobserver-auth=fifo:/tmp/pool.fifo"
    );
    assert_eq!(
        cargo_makeflags("3,4"),
        "-j --jobserver-fds=3,4 --jobserver-auth=3,4"
    );
}

/// A script that fails shows its whole output, warnings included, as Cargo does.
#[test]
fn a_failing_build_script_shows_its_output() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("proj");
    project(&root);
    fs::write(
        root.join("build.rs"),
        "fn main() {\n    println!(\"cargo:warning=about to fail\");\n    \
         eprintln!(\"the reason\");\n    std::process::exit(3);\n}\n",
    )
    .unwrap();
    let out = mitts(&root, "target", &["build"]);
    let log = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(!out.status.success(), "{log}");
    assert!(
        log.contains("cargo:warning=about to fail"),
        "stdout shown: {log}"
    );
    assert!(log.contains("the reason"), "stderr shown: {log}");
}

/// A pool address with a space (cache path with spaces) cannot be written into MAKEFLAGS:
/// the script then runs with NUM_JOBS only, and the build still works. (Unix: there the
/// pool is a FIFO in the cache; Windows names a semaphore, which has no spaces.)
#[cfg(unix)]
#[test]
fn a_cache_path_with_spaces_keeps_the_pool_out_of_makeflags() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("proj");
    project(&root);
    let cache = temp.path().join("cache with spaces");
    let out = mitts_cache(&root, &cache, "target", &["build", "-j", "3"]);
    let log = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(out.status.success(), "{log}");
    assert!(
        warnings(&log).iter().any(|w| w == "makeflags=[]"),
        "no CARGO_MAKEFLAGS for this pool: {log}"
    );
}

/// Cargo shows a fresh script's warnings on every build. Mitts' later builds in the same
/// target come from the cache and then from the fast build lock; each shows them once.
#[test]
fn warnings_are_shown_on_every_build_including_fast_lock_hits() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("proj");
    project(&root);
    let mut last = String::new();
    for run in 1..=4 {
        let out = mitts_fast_lock(&root, &["build", "-v"]);
        let log = String::from_utf8_lossy(&out.stderr).to_string();
        assert!(out.status.success(), "run {run}: {log}");
        assert_eq!(warnings(&log).len(), 2, "run {run}: {log}");
        last = log;
    }
    // The lock answers once an all-hit build recorded it: by the fourth run at the latest.
    assert!(
        !last.contains("Running `"),
        "the last run came from the fast lock, no rustc or script ran: {last}"
    );
}

/// A fast build lock recorded under another build-script protocol is stale: the
/// fingerprint names the protocol.
#[test]
fn the_fast_lock_fingerprint_names_the_build_script_protocol() {
    use mitts::adapter::rust::build_script_keys::PROTOCOL;
    assert_eq!(PROTOCOL, "build_script_v11");
    let dir = TempDir::new().unwrap();
    let manifest = dir.path().join("Cargo.toml");
    let opt = mitts::adapter::RustBuildOptions::default();
    let a = mitts::adapter::rust::builder::compute_options_hash(&opt, dir.path(), &manifest);
    let b = mitts::adapter::rust::builder::compute_options_hash(&opt, dir.path(), &manifest);
    assert_eq!(a, b, "deterministic");
}
