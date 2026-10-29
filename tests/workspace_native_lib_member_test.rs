/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! `--workspace` must cover a member whose library declares native crate types. regex's C
//! API crate `rure` (`crate-type = ["staticlib", "cdylib", "rlib"]`) was planned with the
//! crate-type list as its unit kind and never selected as a root, so `mitts clippy
//! --workspace` exited 0 while Cargo reported deny-level lint errors in it.

use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

fn write_project(dir: &Path) {
    for sub in ["core/src", "capi/src"] {
        fs::create_dir_all(dir.join(sub)).unwrap();
    }
    fs::write(
        dir.join("Cargo.toml"),
        "[workspace]\nmembers = [\"core\", \"capi\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    fs::write(
        dir.join("core/Cargo.toml"),
        "[package]\nname = \"core-lib\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(dir.join("core/src/lib.rs"), "pub fn one() -> u32 { 1 }\n").unwrap();
    fs::write(
        dir.join("capi/Cargo.toml"),
        "[package]\nname = \"capi-ffi\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
         [lib]\nname = \"capi\"\ncrate-type = [\"staticlib\", \"cdylib\", \"rlib\"]\n\n\
         [dependencies]\ncore-lib = { path = \"../core\" }\n",
    )
    .unwrap();
    // A type error only this member has: a workspace check that skips it exits 0.
    fs::write(
        dir.join("capi/src/lib.rs"),
        "#[no_mangle]\npub extern \"C\" fn capi_one() -> u32 { let wrong: u32 = \"one\"; core_lib::one() + wrong }\n",
    )
    .unwrap();
}

#[test]
fn workspace_check_covers_a_member_with_native_crate_types() {
    let cache = TempDir::new().unwrap();
    let dir = TempDir::new().unwrap();
    write_project(dir.path());
    let out = Command::new(env!("CARGO_BIN_EXE_mitts"))
        .args(["check", "--workspace", "--manifest-path"])
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
    assert!(
        !out.status.success(),
        "the C API member is part of --workspace, so its type error fails the check:\n{text}"
    );
    assert!(text.contains("capi"), "the failure names the member:\n{text}");
}
