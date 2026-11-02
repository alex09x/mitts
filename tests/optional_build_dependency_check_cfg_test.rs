/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! An optional build-dependency has an implicit feature that Cargo declares to rustc with
//! `--check-cfg cfg(feature, values(...))`. tonic's examples package gates its build script
//! on one (`#[cfg(feature = "protoc-gen-rust-grpc")]`); Mitts listed only the normal
//! dependency tables, so clippy warned `unexpected cfg condition value` under Mitts alone
//! (tonic: INVALID_L5 on the rig, 0.3.17).

use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

fn write_project(dir: &Path) {
    for sub in ["app/src", "helper/src"] {
        fs::create_dir_all(dir.join(sub)).unwrap();
    }
    fs::write(
        dir.join("app/Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\nbuild = \"build.rs\"\n\n\
         [build-dependencies]\nhelper = { path = \"../helper\", optional = true }\n",
    )
    .unwrap();
    fs::write(
        dir.join("app/build.rs"),
        "fn main() {\n    #[cfg(feature = \"helper\")]\n    helper::touch();\n    println!(\"cargo:rerun-if-changed=build.rs\");\n}\n",
    )
    .unwrap();
    fs::write(
        dir.join("app/src/lib.rs"),
        "#[cfg(feature = \"helper\")]\npub const WITH_HELPER: bool = true;\npub fn f() {}\n",
    )
    .unwrap();
    fs::write(
        dir.join("helper/Cargo.toml"),
        "[package]\nname = \"helper\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(dir.join("helper/src/lib.rs"), "pub fn touch() {}\n").unwrap();
}

#[test]
fn an_optional_build_dependency_is_a_declared_feature_value() {
    let cache = TempDir::new().unwrap();
    let dir = TempDir::new().unwrap();
    write_project(dir.path());
    for cmd in ["check", "clippy"] {
        let out = Command::new(env!("CARGO_BIN_EXE_mitts"))
            .arg(cmd)
            .arg("--manifest-path")
            .arg(dir.path().join("app/Cargo.toml"))
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
        assert!(out.status.success(), "mitts {cmd}:\n{text}");
        assert!(
            !text.contains("unexpected `cfg` condition value"),
            "mitts {cmd} warns about the optional build-dependency's feature:\n{text}"
        );
    }
}
