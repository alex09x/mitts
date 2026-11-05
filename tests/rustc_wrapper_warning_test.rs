/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A rustc wrapper in the cargo config (a global `~/.cargo/config.toml` with sccache is the
//! usual case) makes Mitts skip its cache, and used to say nothing: every run recompiled and the
//! user had no hint why (issue 53). The build now says so and how to turn the wrapper off, and an
//! empty `RUSTC_WRAPPER` / `CARGO_BUILD_RUSTC_WRAPPER` clears it.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output};
use tempfile::TempDir;

fn write(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

fn project() -> TempDir {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    write(
        &root.join("Cargo.toml"),
        "[package]\nname = \"wrapped\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write(&root.join("src/lib.rs"), "pub fn f() -> u32 { 1 }\n");
    // A wrapper that runs the compiler it is given.
    let wrapper = root.join("wrap.sh");
    write(&wrapper, "#!/bin/sh\nexec \"$@\"\n");
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
    write(
        &root.join(".cargo/config.toml"),
        &format!("[build]\nrustc-wrapper = \"{}\"\n", wrapper.display()),
    );
    dir
}

fn check(dir: &TempDir, env: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mitts"));
    command
        .current_dir(dir.path())
        .arg("check")
        .arg("--cache-dir")
        .arg(dir.path().join("cache"))
        .env("CARGO_HOME", dir.path().join("cargo-home"))
        // An inherited variable, even an empty one, takes precedence over the config file.
        .env_remove("RUSTC_WRAPPER")
        .env_remove("CARGO_BUILD_RUSTC_WRAPPER")
        .env_remove("RUSTC_WORKSPACE_WRAPPER")
        .env_remove("CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER");
    for (key, value) in env {
        command.env(key, value);
    }
    command.output().unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn a_configured_wrapper_is_reported_as_the_reason_the_build_is_not_cached() {
    let dir = project();
    let output = check(&dir, &[]);
    assert!(output.status.success(), "{}", stderr(&output));
    let text = stderr(&output);
    assert!(
        text.contains("[mitts::warn] a rustc wrapper is configured"),
        "{text}"
    );
    assert!(text.contains("wrap.sh"), "{text}");
    assert!(text.contains("not cached"), "{text}");
}

#[test]
fn an_empty_wrapper_variable_clears_it_and_the_build_is_cached_without_a_warning() {
    let dir = project();
    for variable in ["CARGO_BUILD_RUSTC_WRAPPER", "RUSTC_WRAPPER"] {
        let output = check(&dir, &[(variable, "")]);
        assert!(output.status.success(), "{}", stderr(&output));
        assert!(
            !stderr(&output).contains("a rustc wrapper is configured"),
            "{variable}: {}",
            stderr(&output)
        );
    }
    // The second run is a full cache hit.
    let again = check(&dir, &[("CARGO_BUILD_RUSTC_WRAPPER", "")]);
    assert!(
        stderr(&again).contains("compiled=0 cached="),
        "{}",
        stderr(&again)
    );
}

#[test]
fn sccache_does_not_turn_the_cache_off_and_is_not_run() {
    let dir = project();
    // The same project, with a wrapper named sccache: it records that it ran.
    let wrapper = dir.path().join("sccache");
    let ran = dir.path().join("sccache-ran.log");
    write(
        &wrapper,
        &format!("#!/bin/sh\necho ran >> '{}'\nexec \"$@\"\n", ran.display()),
    );
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
    write(
        &dir.path().join(".cargo/config.toml"),
        &format!("[build]\nrustc-wrapper = \"{}\"\n", wrapper.display()),
    );

    let first = check(&dir, &[]);
    assert!(first.status.success(), "{}", stderr(&first));
    assert!(
        !stderr(&first).contains("a rustc wrapper is configured"),
        "{}",
        stderr(&first)
    );
    let again = check(&dir, &[]);
    assert!(
        stderr(&again).contains("compiled=0 cached="),
        "the repeat is a cache hit: {}",
        stderr(&again)
    );
    assert!(!ran.exists(), "Mitts compiles without sccache");
}

#[test]
fn without_a_wrapper_there_is_no_warning() {
    let dir = project();
    fs::remove_file(dir.path().join(".cargo/config.toml")).unwrap();
    let output = check(&dir, &[]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(
        !stderr(&output).contains("[mitts::warn] a rustc wrapper"),
        "{}",
        stderr(&output)
    );
}
