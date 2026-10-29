/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Cargo excludes a dependency only through `[target.'cfg(...)']` tables, never by its
//! name. chrono's test suite depends unconditionally on `windows-bindgen`, a code generator
//! that builds on every host; a name-based platform list dropped it from the plan on Linux
//! and macOS and the test failed with "unresolved import `windows_bindgen`". The same list
//! hid `core-foundation`, `js-sys` and friends whenever a manifest named them unconditionally.

use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

fn write_project(dir: &Path) {
    for sub in ["app/src", "app/tests", "windows-bindgen/src", "core-foundation/src"] {
        fs::create_dir_all(dir.join(sub)).unwrap();
    }
    fs::write(
        dir.join("Cargo.toml"),
        "[workspace]\nmembers = [\"app\", \"windows-bindgen\", \"core-foundation\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    fs::write(
        dir.join("app/Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
         [dependencies]\ncore-foundation = { path = \"../core-foundation\" }\n\n\
         [dev-dependencies]\nwindows-bindgen = { path = \"../windows-bindgen\" }\n",
    )
    .unwrap();
    fs::write(
        dir.join("app/src/lib.rs"),
        "pub fn answer() -> u32 { core_foundation::forty() + 2 }\n",
    )
    .unwrap();
    fs::write(
        dir.join("app/tests/gen.rs"),
        "#[test]\nfn uses_the_generator() { assert_eq!(windows_bindgen::bindgen(), 7); assert_eq!(app::answer(), 42); }\n",
    )
    .unwrap();
    fs::write(
        dir.join("windows-bindgen/Cargo.toml"),
        "[package]\nname = \"windows-bindgen\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(dir.join("windows-bindgen/src/lib.rs"), "pub fn bindgen() -> u32 { 7 }\n").unwrap();
    fs::write(
        dir.join("core-foundation/Cargo.toml"),
        "[package]\nname = \"core-foundation\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(dir.join("core-foundation/src/lib.rs"), "pub fn forty() -> u32 { 40 }\n").unwrap();
}

#[test]
fn unconditional_dependencies_named_like_platform_crates_are_built() {
    let cache = TempDir::new().unwrap();
    let dir = TempDir::new().unwrap();
    write_project(dir.path());
    let out = Command::new(env!("CARGO_BIN_EXE_mitts"))
        .args(["test", "--workspace", "--tests", "--manifest-path"])
        .arg(dir.path().join("Cargo.toml"))
        .arg("--cache-dir")
        .arg(cache.path())
        .env("CI", "true")
        .env_remove("MITTS_CACHE_DIR")
        .output()
        .expect("run mitts");
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "mitts test links both path crates whatever their names:\n{text}");
    assert!(text.contains("uses_the_generator"), "the integration test ran:\n{text}");
}
