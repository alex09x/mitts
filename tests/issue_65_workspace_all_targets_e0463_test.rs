/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Regression tests for issue #65: `mitts check --workspace --all-targets` failing with
//! `error[E0463]: can't find crate` on unit-test and integration-test targets in multi-crate
//! workspaces (such as `rust-analyzer`).

use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use tempfile::TempDir;

fn run_mitts(root: &Path, cache: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mitts"))
        .current_dir(root)
        .args(args)
        .args(["--offline", "--cache-dir"])
        .arg(cache)
        .arg("--target-dir")
        .arg(root.join("target"))
        .env("CARGO_HOME", root.join("cargo-home"))
        .env_remove("MITTS_CACHE_DIR")
        .env_remove("RUSTC_WRAPPER")
        .env_remove("RUSTC_WORKSPACE_WRAPPER")
        .env_remove("CARGO_BUILD_RUSTC_WRAPPER")
        .env_remove("CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER")
        .output()
        .expect("run mitts")
}

fn assert_ok(out: &Output, step: &str) {
    if !out.status.success() {
        panic!(
            "{step} failed (status {:?}):\n--- stdout ---\n{}\n--- stderr ---\n{}",
            out.status.code(),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        );
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("E0463"),
        "{step} unexpectedly emitted E0463:\n{stderr}"
    );
}

fn write_ra_shaped_workspace(ws: &Path, sub_fn_body: &str) {
    fs::create_dir_all(ws.join("crates/common/src")).unwrap();
    fs::create_dir_all(ws.join("crates/hir/src")).unwrap();
    fs::create_dir_all(ws.join("crates/hir/tests")).unwrap();
    fs::create_dir_all(ws.join("crates/ide_db/src")).unwrap();
    fs::create_dir_all(ws.join("crates/ide_assists/src")).unwrap();
    fs::create_dir_all(ws.join("crates/ra_cli/src")).unwrap();

    fs::write(
        ws.join("Cargo.toml"),
        r#"[workspace]
resolver = "2"
members = [
    "crates/common",
    "crates/hir",
    "crates/ide_db",
    "crates/ide_assists",
    "crates/ra_cli",
]
"#,
    )
    .unwrap();

    // `common` exposes `pub struct Span(pub u32)` so downstream crates returning `common::Span`
    // in public signatures force `rustc_metadata::locator` to load `common`'s `.rmeta` via
    // `-L dependency=` even in crates (`ide_assists`, `ra_cli`) that do not declare `common`
    // directly in `[dependencies]`.
    fs::write(
        ws.join("crates/common/Cargo.toml"),
        "[package]\nname = \"common\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(
        ws.join("crates/common/src/lib.rs"),
        "pub mod sub;\n#[derive(Clone, Copy, Debug, PartialEq, Eq)]\npub struct Span(pub u32);\npub fn base() -> Span { Span(sub::val()) }\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn base_works() { assert!(super::base().0 > 0); }\n}\n",
    )
    .unwrap();
    fs::write(ws.join("crates/common/src/sub.rs"), sub_fn_body).unwrap();
    fs::write(
        ws.join("crates/common/src/main.rs"),
        "fn main() { println!(\"{}\", common::base().0); }\n",
    )
    .unwrap();

    fs::write(
        ws.join("crates/hir/Cargo.toml"),
        "[package]\nname = \"hir\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\ncommon = { path = \"../common\" }\n",
    )
    .unwrap();
    fs::write(
        ws.join("crates/hir/src/lib.rs"),
        "pub use common::Span;\npub fn hir_span() -> Span { Span(common::base().0 + 1) }\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn hir_unit() { assert_eq!(super::hir_span().0, common::base().0 + 1); }\n}\n",
    )
    .unwrap();
    fs::write(
        ws.join("crates/hir/tests/integration.rs"),
        "#[test]\nfn hir_integ() { assert_eq!(hir::hir_span().0, common::base().0 + 1); }\n",
    )
    .unwrap();

    // `ide_db` depends only on `hir` (transitive dependency on `common` via re-exported `Span`).
    fs::write(
        ws.join("crates/ide_db/Cargo.toml"),
        "[package]\nname = \"ide_db\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nhir = { path = \"../hir\" }\n",
    )
    .unwrap();
    fs::write(
        ws.join("crates/ide_db/src/lib.rs"),
        "pub use hir::Span;\npub fn db_span() -> Span { hir::hir_span() }\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn db_unit() { assert_eq!(super::db_span().0, hir::hir_span().0); }\n}\n",
    )
    .unwrap();

    // `ide_assists` depends on `hir` and `ide_db` (two transitive paths to `common::Span`).
    fs::write(
        ws.join("crates/ide_assists/Cargo.toml"),
        "[package]\nname = \"ide_assists\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nhir = { path = \"../hir\" }\nide_db = { path = \"../ide_db\" }\n",
    )
    .unwrap();
    fs::write(
        ws.join("crates/ide_assists/src/lib.rs"),
        "pub fn assist_span() -> ide_db::Span { ide_db::db_span() }\npub fn assist_val() -> u32 { hir::hir_span().0 + ide_db::db_span().0 }\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn assist_unit() {\n        assert_eq!(super::assist_val(), hir::hir_span().0 + ide_db::db_span().0);\n        assert!(super::assist_span().0 > 0);\n    }\n}\n",
    )
    .unwrap();

    // `ra_cli` depends ONLY on `ide_assists` (3-hop transitive dependency on `common`, `hir`, `ide_db`).
    fs::write(
        ws.join("crates/ra_cli/Cargo.toml"),
        "[package]\nname = \"ra_cli\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nide_assists = { path = \"../ide_assists\" }\n",
    )
    .unwrap();
    fs::write(
        ws.join("crates/ra_cli/src/main.rs"),
        "fn main() { println!(\"{}\", ide_assists::assist_span().0); }\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn cli_unit() { assert_eq!(ide_assists::assist_span().0 * 2, ide_assists::assist_val()); }\n}\n",
    )
    .unwrap();
}

#[test]
fn workspace_all_targets_check_resolves_unit_test_deps_across_rebuilds() {
    let tmp = TempDir::new().unwrap();
    let cache = tmp.path().join("cache");
    let ws = tmp.path().join("ws");

    write_ra_shaped_workspace(&ws, "pub fn val() -> u32 { 10 }\n");

    let first = run_mitts(&ws, &cache, &["check", "--workspace", "--all-targets"]);
    assert_ok(&first, "cold check --workspace --all-targets");

    // Edit non-root submodule `common/src/sub.rs` (changing `common`'s SVH while its root
    // `src/lib.rs` is unchanged) and re-run `--workspace --all-targets` against the warm cache.
    fs::write(
        ws.join("crates/common/src/sub.rs"),
        "pub fn val() -> u32 { 42 }\npub fn extra() -> u32 { 7 }\n",
    )
    .unwrap();

    let second = run_mitts(&ws, &cache, &["check", "--workspace", "--all-targets"]);
    assert_ok(&second, "incremental check --workspace --all-targets");

    let third = run_mitts(&ws, &cache, &["check", "--workspace", "--all-targets"]);
    assert_ok(&third, "warm check --workspace --all-targets");

    // Pure binary crates (`ra_cli`) must never publish bogus `libra_cli-*.rmeta` entries into CAS
    // named artifact directories during `check --all-targets`.
    let meta_dir = cache.join("cas").join("artifacts-meta");
    if meta_dir.is_dir() {
        for entry in fs::read_dir(&meta_dir).unwrap().flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            assert!(
                !name.starts_with("libra_cli"),
                "binary/test check unit polluted cas/artifacts-meta with {name}"
            );
        }
    }

    let test_no_run = run_mitts(&ws, &cache, &["test", "--workspace", "--no-run"]);
    assert_ok(&test_no_run, "test --workspace --no-run");
}

#[test]
fn workspace_all_targets_check_isolates_build_script_action_keys() {
    let tmp = TempDir::new().unwrap();
    let cache = tmp.path().join("cache");
    let ws = tmp.path().join("ws_bd");
    fs::create_dir_all(ws.join("sys_dep/src")).unwrap();
    fs::create_dir_all(ws.join("middle/src")).unwrap();
    fs::create_dir_all(ws.join("consumer/src")).unwrap();

    fs::write(
        ws.join("Cargo.toml"),
        "[workspace]\nresolver = \"2\"\nmembers = [\"sys_dep\", \"middle\", \"consumer\"]\n",
    )
    .unwrap();
    fs::write(
        ws.join("sys_dep/Cargo.toml"),
        "[package]\nname = \"sys_dep\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(
        ws.join("sys_dep/build.rs"),
        r#"fn main() {
    println!("cargo:rerun-if-env-changed=TEST_CFG_TOKEN");
    let tok = std::env::var("TEST_CFG_TOKEN").unwrap_or_else(|_| "alpha".into());
    println!("cargo:rustc-env=GENERATED_TOKEN={tok}");
}
"#,
    )
    .unwrap();
    fs::write(
        ws.join("sys_dep/src/lib.rs"),
        "#[derive(Clone, Copy, Debug, PartialEq, Eq)]\npub struct Token(pub &'static str);\npub fn token() -> Token { Token(env!(\"GENERATED_TOKEN\")) }\n",
    )
    .unwrap();
    fs::write(
        ws.join("middle/Cargo.toml"),
        "[package]\nname = \"middle\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nsys_dep = { path = \"../sys_dep\" }\n",
    )
    .unwrap();
    fs::write(
        ws.join("middle/src/lib.rs"),
        "pub fn wrap() -> sys_dep::Token { sys_dep::token() }\n",
    )
    .unwrap();
    // `consumer` depends only on `middle` so `sys_dep`'s `:bd:`-disambiguated `.rmeta` is loaded
    // transitively via `-L dependency=` on both `consumer` lib and `consumer` unit tests.
    fs::write(
        ws.join("consumer/Cargo.toml"),
        "[package]\nname = \"consumer\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nmiddle = { path = \"../middle\" }\n",
    )
    .unwrap();
    fs::write(
        ws.join("consumer/src/lib.rs"),
        "pub fn get() -> &'static str { middle::wrap().0 }\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn check_tok() { assert!(!super::get().is_empty()); }\n}\n",
    )
    .unwrap();

    for token in ["alpha", "beta_different_svh"] {
        let out = Command::new(env!("CARGO_BIN_EXE_mitts"))
            .current_dir(&ws)
            .args([
                "check",
                "--workspace",
                "--all-targets",
                "--offline",
                "--cache-dir",
            ])
            .arg(&cache)
            .arg("--target-dir")
            .arg(ws.join("target"))
            .env("CARGO_HOME", ws.join("cargo-home"))
            .env("TEST_CFG_TOKEN", token)
            .env_remove("MITTS_CACHE_DIR")
            .output()
            .unwrap();
        assert_ok(&out, &format!("check with TEST_CFG_TOKEN={token}"));
    }
}
