/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A virtual workspace without a `resolver` key uses resolver 1, which unifies features
//! between the host and target sides. argh's `argh_shared` is used by `argh_derive` (a
//! proc-macro, built for the host) and by `argh`, which turns its `serde` feature on: the
//! host copy must then be built with `serde` too, and `serde` must be built for the host
//! to link it. Mitts unified the feature names but never activated the dependency on the
//! host side, so the host unit was compiled with `--cfg feature="serde"` and no crate.

use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

/// `app` → `shared` (feature `extra` → optional `helper`); `pm` (proc-macro) → `shared`.
fn write_project(dir: &Path) {
    for sub in ["app/src", "shared/src", "helper/src", "pm/src"] {
        fs::create_dir_all(dir.join(sub)).unwrap();
    }
    fs::write(
        dir.join("Cargo.toml"),
        "[workspace]\nmembers = [\"app\", \"shared\", \"helper\", \"pm\"]\n",
    )
    .unwrap();
    fs::write(
        dir.join("app/Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
         [dependencies]\nshared = { path = \"../shared\", features = [\"extra\"] }\npm = { path = \"../pm\" }\n",
    )
    .unwrap();
    fs::write(
        dir.join("app/src/lib.rs"),
        "pub fn v() -> u64 { shared::val() + pm::one!() }\n",
    )
    .unwrap();
    fs::write(
        dir.join("shared/Cargo.toml"),
        "[package]\nname = \"shared\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
         [dependencies]\nhelper = { path = \"../helper\", optional = true }\n\n\
         [features]\nextra = [\"dep:helper\"]\n",
    )
    .unwrap();
    fs::write(
        dir.join("shared/src/lib.rs"),
        "#[cfg(feature = \"extra\")]\npub fn val() -> u64 { helper::seven() }\n\
         #[cfg(not(feature = \"extra\"))]\npub fn val() -> u64 { 1 }\n",
    )
    .unwrap();
    fs::write(
        dir.join("helper/Cargo.toml"),
        "[package]\nname = \"helper\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(dir.join("helper/src/lib.rs"), "pub fn seven() -> u64 { 7 }\n").unwrap();
    fs::write(
        dir.join("pm/Cargo.toml"),
        "[package]\nname = \"pm\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
         [lib]\nproc-macro = true\n\n[dependencies]\nshared = { path = \"../shared\" }\n",
    )
    .unwrap();
    fs::write(
        dir.join("pm/src/lib.rs"),
        "use proc_macro::TokenStream;\n\
         #[proc_macro]\npub fn one(_: TokenStream) -> TokenStream {\n\
         \x20   let _ = shared::val();\n    \"1u64\".parse().unwrap()\n}\n",
    )
    .unwrap();
}

#[test]
fn resolver_1_builds_the_host_copy_with_the_unified_optional_dependency() {
    let cache = TempDir::new().unwrap();
    let dir = TempDir::new().unwrap();
    write_project(dir.path());
    for args in [&["check", "--workspace"][..], &["build"][..]] {
        let cmd = args[0];
        let out = Command::new(env!("CARGO_BIN_EXE_mitts"))
            .args(args)
            .arg("--manifest-path")
            .arg(dir.path().join("Cargo.toml"))
            .arg("--cache-dir")
            .arg(cache.path())
            .env("CI", "true")
            .env_remove("MITTS_CACHE_DIR")
            .output()
            .expect("run mitts");
        assert!(
            out.status.success(),
            "mitts {cmd} --workspace links the host `helper` for the proc-macro's `shared`:\n{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
