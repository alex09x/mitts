/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! `[target.'cfg(...)'.dependencies]` is evaluated with the build's rustflags, as Cargo asks
//! `rustc --print cfg` with them: a dependency under `cfg(target_feature = "crt-static")` is
//! in the build when `-C target-feature=+crt-static` comes from RUSTFLAGS, and out without.
// crt-static is a stable target feature on Linux gnu and Windows MSVC.
#![cfg(any(all(target_os = "linux", target_env = "gnu"), target_env = "msvc"))]

use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use tempfile::TempDir;

fn write(path: impl AsRef<Path>, contents: &str) {
    let path = path.as_ref();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn project(root: &Path) {
    write(
        root.join("Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
         [target.'cfg(target_feature = \"crt-static\")'.dependencies]\n\
         only_static = { path = \"only_static\" }\n",
    );
    write(
        root.join("src/lib.rs"),
        "#[cfg(target_feature = \"crt-static\")]\n\
         pub fn which() -> &'static str { only_static::NAME }\n\
         #[cfg(not(target_feature = \"crt-static\"))]\n\
         pub fn which() -> &'static str { \"dynamic\" }\n",
    );
    write(
        root.join("only_static/Cargo.toml"),
        "[package]\nname = \"only_static\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write(
        root.join("only_static/src/lib.rs"),
        "pub const NAME: &str = \"static\";\n",
    );
}

fn mitts(root: &Path, args: &[&str], rustflags: Option<&str>) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mitts"));
    cmd.args(args).arg("--cache-dir").arg(root.join("cache"));
    if args[0] == "build" {
        cmd.arg("--target-dir").arg(root.join("target"));
    }
    cmd.current_dir(root)
        .env("CARGO_HOME", root.join("cargo-home"))
        .env_remove("MITTS_CACHE_DIR");
    for name in [
        "CARGO_TARGET_DIR",
        "CARGO_BUILD_TARGET",
        "CARGO_ENCODED_RUSTFLAGS",
        "RUSTFLAGS",
        "CARGO_BUILD_RUSTFLAGS",
        "RUSTC_WRAPPER",
        "RUSTC_WORKSPACE_WRAPPER",
        "CARGO_BUILD_RUSTC_WRAPPER",
    ] {
        cmd.env_remove(name);
    }
    if let Some(flags) = rustflags {
        cmd.env("RUSTFLAGS", flags);
    }
    cmd.output().expect("run Mitts")
}

#[test]
fn a_target_feature_from_rustflags_selects_a_cfg_dependency() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("proj");
    project(&root);

    let with = mitts(
        &root,
        &["build", "-v"],
        Some("-C target-feature=+crt-static"),
    );
    let log = String::from_utf8_lossy(&with.stderr);
    assert!(with.status.success(), "{log}");
    assert!(
        log.contains("only_static"),
        "the dependency is built: {log}"
    );

    let tree = mitts(&root, &["tree"], Some("-C target-feature=+crt-static"));
    let out = String::from_utf8_lossy(&tree.stdout);
    assert!(
        tree.status.success(),
        "{}",
        String::from_utf8_lossy(&tree.stderr)
    );
    assert!(out.contains("only_static"), "{out}");

    let without = mitts(&root, &["build", "-v"], None);
    let log = String::from_utf8_lossy(&without.stderr);
    assert!(without.status.success(), "{log}");
    assert!(
        !log.contains("--crate-name only_static"),
        "not in the build without the feature: {log}"
    );
}
