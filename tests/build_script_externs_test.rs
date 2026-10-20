/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A build script sees only the package's `[build-dependencies]`, as in Cargo.
//! Regular dependencies must not reach its link line or its cache key: keying a
//! script on the bytes of unrelated artifacts re-ran scripts after every cache
//! restore and hashed every dependency rlib on every build. A warm build must
//! serve every script from the cache.

use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

fn write(path: &Path, content: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

/// `app` has a build script, a regular dependency `regular` (a library) and a
/// build-dependency `bdep` (a library used by the script).
fn workspace(root: &Path) {
    write(
        &root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"app\", \"regular\", \"bdep\"]\nresolver = \"2\"\n",
    );
    write(
        &root.join("regular/Cargo.toml"),
        "[package]\nname = \"regular\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write(
        &root.join("regular/src/lib.rs"),
        "pub fn regular() -> u32 { 7 }\n",
    );
    write(
        &root.join("bdep/Cargo.toml"),
        "[package]\nname = \"bdep\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write(&root.join("bdep/src/lib.rs"), "pub fn answer() -> u32 { 42 }\n");
    write(
        &root.join("app/Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nregular = { path = \"../regular\" }\n\n[build-dependencies]\nbdep = { path = \"../bdep\" }\n",
    );
    write(
        &root.join("app/build.rs"),
        "fn main() {\n    println!(\"cargo:rustc-env=BDEP_ANSWER={}\", bdep::answer());\n    println!(\"cargo:rerun-if-changed=build.rs\");\n}\n",
    );
    write(
        &root.join("app/src/main.rs"),
        "fn main() { println!(\"{} {}\", env!(\"BDEP_ANSWER\"), regular::regular()); }\n",
    );
}

fn mitts_build(root: &Path, cache: &Path) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_mitts"))
        .arg("build")
        .arg("--manifest-path")
        .arg(root.join("app/Cargo.toml"))
        .arg("--cache-dir")
        .arg(cache)
        .env("MITTS_TIMINGS", "1")
        .env("MITTS_LOG_COMPILE", "1")
        .env("CI", "true")
        .output()
        .unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn script_events(stderr: &str, event: &str) -> Vec<String> {
    stderr
        .lines()
        .filter_map(|l| l.strip_prefix(&format!("[mitts::build_script] {event} ")))
        .map(str::to_owned)
        .collect()
}

#[test]
fn build_script_key_and_externs_use_build_dependencies_only() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("ws");
    workspace(&root);
    let cache = temp.path().join("cache");

    let (ok, cold) = mitts_build(&root, &cache);
    assert!(ok, "cold build failed:\n{cold}");
    let key_line = cold
        .lines()
        .find(|l| l.starts_with("[mitts::build_script_key] app "))
        .unwrap_or_else(|| panic!("no key line for app:\n{cold}"));
    assert!(
        key_line.contains("build_dep:bdep="),
        "the build-dependency must key the script: {key_line}"
    );
    assert!(
        !key_line.contains("build_dep:regular="),
        "a regular dependency must not key the script: {key_line}"
    );
    assert!(
        cold.lines()
            .any(|l| l.starts_with("[mitts::build_script] miss app ")),
        "the cold build must run app's script:\n{cold}"
    );

    let (ok, warm) = mitts_build(&root, &cache);
    assert!(ok, "warm build failed:\n{warm}");
    let misses = script_events(&warm, "miss");
    assert!(
        misses.is_empty(),
        "a warm build must not re-run any build script, re-ran: {misses:?}\n{warm}"
    );
    assert!(
        script_events(&warm, "hit")
            .iter()
            .any(|l| l.starts_with("app ")),
        "app's script must be served from the cache:\n{warm}"
    );
}
