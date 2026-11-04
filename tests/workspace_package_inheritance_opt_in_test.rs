/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Cargo fills a `[package]` field from `[workspace.package]` only where the member wrote
//! `field.workspace = true`. winnow's fuzz member declares no `rust-version`; Mitts gave it
//! the workspace's 1.65.0 anyway, so clippy's `incompatible_msrv` fired on the fuzz targets
//! under Mitts and not under Cargo (winnow: INVALID_L5 on the rig). Each member's build
//! script writes the `CARGO_PKG_RUST_VERSION` it was given, and the binary prints it.

use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

fn write_member(dir: &Path, name: &str, package_tail: &str) {
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(
        dir.join("Cargo.toml"),
        format!(
            "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n{package_tail}\n"
        ),
    )
    .unwrap();
    fs::write(
        dir.join("build.rs"),
        "fn main() {\n    let out = std::path::PathBuf::from(std::env::var(\"OUT_DIR\").unwrap());\n\
         \x20   let rv = std::env::var(\"CARGO_PKG_RUST_VERSION\").unwrap_or_default();\n\
         \x20   std::fs::write(out.join(\"rv.txt\"), format!(\"[{rv}]\")).unwrap();\n\
         \x20   println!(\"cargo:rerun-if-changed=build.rs\");\n}\n",
    )
    .unwrap();
    fs::write(
        dir.join("src/main.rs"),
        "fn main() { print!(\"{}\", include_str!(concat!(env!(\"OUT_DIR\"), \"/rv.txt\"))); }\n",
    )
    .unwrap();
}

#[test]
fn a_member_inherits_the_workspace_rust_version_only_when_it_opts_in() {
    let cache = TempDir::new().unwrap();
    let temp = TempDir::new().unwrap();
    let ws = temp.path();
    fs::write(
        ws.join("Cargo.toml"),
        "[workspace]\nmembers = [\"opted\", \"plain\"]\nresolver = \"2\"\n\n\
         [workspace.package]\nrust-version = \"1.65.0\"\n",
    )
    .unwrap();
    write_member(&ws.join("opted"), "opted", "rust-version.workspace = true");
    write_member(&ws.join("plain"), "plain", "");

    let out = Command::new(env!("CARGO_BIN_EXE_mitts"))
        .arg("build")
        .arg("--manifest-path")
        .arg(ws.join("Cargo.toml"))
        .arg("--cache-dir")
        .arg(cache.path())
        .env("CI", "true")
        .env_remove("MITTS_CACHE_DIR")
        .output()
        .expect("run mitts");
    assert!(
        out.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let printed = |bin: &str| {
        let run = Command::new(ws.join("target/debug").join(bin))
            .output()
            .unwrap();
        String::from_utf8_lossy(&run.stdout).to_string()
    };
    assert_eq!(
        printed("opted"),
        "[1.65.0]",
        "opted in: the workspace value"
    );
    assert_eq!(
        printed("plain"),
        "[]",
        "no opt-in: no rust-version, as under Cargo"
    );
}
