/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Under `lto = "fat"` a library that only feeds LTO links needs bitcode and no object code;
//! Cargo builds it with `-C linker-plugin-lto` and so must Mitts (issue 109: uroman spent over
//! an hour generating riscv64 machine code that the final link threw away). A library that
//! also feeds a link without LTO keeps its object code, and the two builds never share a
//! cache entry.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Output;
use tempfile::TempDir;

/// Workspace: `dep` (lib), `app` (bin using dep), `plug` (dylib using dep); release LTO on.
/// Every rustc call is logged by a wrapper. `profile_extra` is appended to the release profile.
fn workspace_with(profile_extra: &str) -> TempDir {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    let write = |rel: &str, text: &str| {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    };
    write(
        "Cargo.toml",
        &format!(
            "[workspace]\nmembers = [\"dep\", \"app\", \"plug\"]\nresolver = \"2\"\n\n\
             [profile.release]\nlto = true\ncodegen-units = 1\n{profile_extra}"
        ),
    );
    let pkg = |name: &str, extra: &str| {
        format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n{extra}")
    };
    write("dep/Cargo.toml", &pkg("dep", ""));
    write("dep/src/lib.rs", "pub fn answer() -> u32 { 6 * 7 }\n");
    let uses_dep = "\n[dependencies]\ndep = { path = \"../dep\" }\n";
    write("app/Cargo.toml", &pkg("app", uses_dep));
    write(
        "app/src/main.rs",
        "fn main() { println!(\"answer={}\", dep::answer()); }\n",
    );
    write(
        "plug/Cargo.toml",
        &pkg(
            "plug",
            &format!("\n[lib]\ncrate-type = [\"dylib\"]\n{uses_dep}"),
        ),
    );
    write(
        "plug/src/lib.rs",
        "pub fn plugged() -> u32 { dep::answer() + 1 }\n",
    );

    let log = root.join("rustc-calls.log");
    let wrapper = root.join("wrap.sh");
    fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nexec \"$@\"\n",
            log.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
    write(
        ".cargo/config.toml",
        &format!("[build]\nrustc-wrapper = \"{}\"\n", wrapper.display()),
    );
    dir
}

fn workspace() -> TempDir {
    workspace_with("")
}

/// The `-C metadata=` value of a rustc command line.
fn metadata_of(call: &str) -> Option<&str> {
    call.split_whitespace()
        .find_map(|arg| arg.strip_prefix("metadata="))
}

fn mitts_release(dir: &Path, cache: &Path, package: &str) -> Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_mitts"))
        .current_dir(dir)
        .args([
            "build",
            "--release",
            "--offline",
            "-p",
            package,
            "--cache-dir",
        ])
        .arg(cache)
        .env("CARGO_HOME", dir.join("cargo-home"))
        .env_remove("RUSTC_WRAPPER")
        .env_remove("RUSTC_WORKSPACE_WRAPPER")
        .env_remove("CARGO_BUILD_RUSTC_WRAPPER")
        .env_remove("CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER")
        .env_remove("RUSTFLAGS")
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .output()
        .unwrap()
}

/// rustc command lines for crate `name`, taken from the wrapper log and then cleared.
fn take_calls(dir: &Path, name: &str) -> Vec<String> {
    let log = dir.join("rustc-calls.log");
    let text = fs::read_to_string(&log).unwrap_or_default();
    let _ = fs::remove_file(&log);
    let needle = format!("--crate-name {name} ");
    text.lines()
        .filter(|line| line.contains(&needle))
        .map(str::to_string)
        .collect()
}

fn assert_success(output: &Output, what: &str) {
    assert!(
        output.status.success(),
        "{what} failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn a_library_that_only_feeds_an_lto_binary_is_built_as_bitcode() {
    let ws = workspace();
    let cache = TempDir::new().unwrap();
    assert_success(
        &mitts_release(ws.path(), cache.path(), "app"),
        "build -p app",
    );

    let dep = take_calls(ws.path(), "dep");
    assert_eq!(dep.len(), 1, "dep compiled once: {dep:?}");
    assert!(
        dep[0].contains("linker-plugin-lto"),
        "dep only feeds an LTO binary, so Cargo passes -C linker-plugin-lto: {}",
        dep[0]
    );
    // The binary itself runs LTO; it is not a bitcode-only unit.
    let app = take_calls(ws.path(), "app");
    assert!(
        app.iter()
            .all(|c| !c.contains("linker-plugin-lto") && c.contains("lto=fat")),
        "{app:?}"
    );

    let bin = ws.path().join("target/release/app");
    let run = std::process::Command::new(&bin).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "answer=42");
}

#[test]
fn a_library_a_dylib_links_keeps_object_code_and_its_own_cache_entry() {
    let ws = workspace();
    let cache = TempDir::new().unwrap();
    assert_success(
        &mitts_release(ws.path(), cache.path(), "app"),
        "build -p app",
    );
    let _ = take_calls(ws.path(), "dep");

    // Same cache: the bitcode-only dep from the first build must not be restored here, as
    // the dylib links it without LTO.
    assert_success(
        &mitts_release(ws.path(), cache.path(), "plug"),
        "build -p plug",
    );
    let dep = take_calls(ws.path(), "dep");
    assert_eq!(
        dep.len(),
        1,
        "dep is compiled again for the dylib rather than restored: {dep:?}"
    );
    assert!(
        !dep[0].contains("linker-plugin-lto"),
        "a dylib consumer needs object code: {}",
        dep[0]
    );
}

#[test]
fn incremental_bitcode_and_object_builds_keep_separate_crate_identities() {
    // Review of #131: with `incremental = true` a workspace library gets a stable
    // `-C metadata`; the bitcode-only and object-code builds must not share it (one rlib
    // file name, one rustc session, thrashed on every consumer switch).
    let ws = workspace_with("incremental = true\n");
    let cache = TempDir::new().unwrap();
    assert_success(
        &mitts_release(ws.path(), cache.path(), "app"),
        "build -p app",
    );
    let bitcode = take_calls(ws.path(), "dep");
    assert_success(
        &mitts_release(ws.path(), cache.path(), "plug"),
        "build -p plug",
    );
    let object = take_calls(ws.path(), "dep");
    assert_eq!(
        (bitcode.len(), object.len()),
        (1, 1),
        "{bitcode:?} {object:?}"
    );
    assert!(bitcode[0].contains("linker-plugin-lto") && bitcode[0].contains("incremental="));
    assert!(!object[0].contains("linker-plugin-lto"));
    let (b, o) = (metadata_of(&bitcode[0]), metadata_of(&object[0]));
    assert!(
        b.is_some(),
        "incremental units carry a stable -C metadata: {}",
        bitcode[0]
    );
    assert_ne!(b, o, "the two builds of dep need distinct crate identities");

    // Back to the binary: still links and runs.
    assert_success(
        &mitts_release(ws.path(), cache.path(), "app"),
        "rebuild -p app",
    );
    let run = std::process::Command::new(ws.path().join("target/release/app"))
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "answer=42");
}
