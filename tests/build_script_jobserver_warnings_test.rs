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
    fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
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
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mitts"));
    cmd.args(args)
        .arg("--cache-dir")
        .arg(root.join("cache"))
        .arg("--target-dir")
        .arg(root.join(target))
        .current_dir(root)
        .env("CARGO_HOME", root.join("cargo-home"))
        .env_remove("MITTS_CACHE_DIR");
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
