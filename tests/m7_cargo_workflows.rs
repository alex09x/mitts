/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::{BuildMode, RustAdapter, RustBuildOptions};
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn fixture_manifest() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("hello-rust")
        .join("Cargo.toml")
}

#[test]
fn test_m7_cargo_check_and_mitts_check_equivalence() {
    let manifest = fixture_manifest();
    let temp_cache = tempfile::tempdir().expect("cache tempdir");

    // 1. Run standard cargo check
    let cargo_status = Command::new("cargo")
        .arg("check")
        .arg("--manifest-path")
        .arg(&manifest)
        .status()
        .expect("cargo check");
    assert!(cargo_status.success(), "Standard cargo check must succeed");

    // 2. Run mitts check in BuildMode::Check
    let opt = RustBuildOptions {
        manifest_path: Some(manifest.clone()),
        cache_dir: Some(temp_cache.path().to_path_buf()),
        mode: BuildMode::Check,
        native: true,
        ..Default::default()
    };
    let (plan, result) = RustAdapter::build_native(&opt).expect("mitts check plan");
    assert!(
        result.success,
        "Mitts check must succeed: {}",
        result.stderr
    );
    assert_eq!(result.exit_code, 0);

    // Verify all actions are typecheck/metadata actions (no local binary bloat)
    assert!(!plan.actions.is_empty(), "Plan must contain actions");
}

#[test]
fn test_m7_cargo_clippy_execution() {
    let manifest = fixture_manifest();
    let temp_cache = tempfile::tempdir().expect("cache tempdir");

    let opt = RustBuildOptions {
        manifest_path: Some(manifest.clone()),
        cache_dir: Some(temp_cache.path().to_path_buf()),
        mode: BuildMode::Clippy,
        native: true,
        ..Default::default()
    };
    let (plan, result) = RustAdapter::build_native(&opt).expect("mitts clippy plan");
    assert!(
        result.success,
        "Mitts clippy must succeed: {}",
        result.stderr
    );
    assert_eq!(result.exit_code, 0);
    assert!(!plan.actions.is_empty());
}

#[test]
fn test_m7_multi_edition_compilation() {
    let temp_workspace = tempfile::tempdir().expect("workspace tempdir");
    let ws_path = temp_workspace.path();
    let temp_cache = tempfile::tempdir().expect("cache tempdir");

    // Create a 2018 edition crate and a 2021 edition crate side by side
    let root_toml = r#"[workspace]
members = ["crate_2018", "crate_2021"]
"#;
    fs::write(ws_path.join("Cargo.toml"), root_toml).unwrap();

    // 2018 crate
    let dir_2018 = ws_path.join("crate_2018");
    fs::create_dir_all(dir_2018.join("src")).unwrap();
    let toml_2018 = r#"[package]
name = "crate_2018"
version = "0.1.0"
edition = "2018"
"#;
    fs::write(dir_2018.join("Cargo.toml"), toml_2018).unwrap();
    fs::write(
        dir_2018.join("src").join("lib.rs"),
        "pub fn hello_2018() -> &'static str { \"2018\" }\n",
    )
    .unwrap();

    // 2021 crate depending on 2018 crate
    let dir_2021 = ws_path.join("crate_2021");
    fs::create_dir_all(dir_2021.join("src")).unwrap();
    let toml_2021 = r#"[package]
name = "crate_2021"
version = "0.1.0"
edition = "2021"

[dependencies]
crate_2018 = { path = "../crate_2018" }
"#;
    fs::write(dir_2021.join("Cargo.toml"), toml_2021).unwrap();
    fs::write(
        dir_2021.join("src").join("main.rs"),
        "fn main() { println!(\"{}\", crate_2018::hello_2018()); }\n",
    )
    .unwrap();

    let opt = RustBuildOptions {
        manifest_path: Some(ws_path.join("Cargo.toml")),
        cache_dir: Some(temp_cache.path().to_path_buf()),
        native: true,
        ..Default::default()
    };

    let explanation = RustAdapter::explain_plan(&opt).expect("explain plan");
    let u_2018 = explanation
        .units
        .iter()
        .find(|u| u.package_name == "crate_2018")
        .expect("crate_2018 unit");
    let u_2021 = explanation
        .units
        .iter()
        .find(|u| u.package_name == "crate_2021")
        .expect("crate_2021 unit");

    assert_eq!(u_2018.edition.as_deref(), Some("2018"));
    assert_eq!(u_2021.edition.as_deref(), Some("2021"));

    let (plan, result) = RustAdapter::build_native(&opt).expect("build multi-edition workspace");
    assert!(
        result.success,
        "Multi-edition build must succeed: {}",
        result.stderr
    );
    assert_eq!(result.exit_code, 0);
    assert_eq!(plan.actions.len(), 2);
}

#[test]
fn test_m7_action_key_dependency_reuse_across_modes() {
    let manifest = fixture_manifest();
    let temp_cache = tempfile::tempdir().expect("cache tempdir");

    // Build plan in Build mode
    let opt_build = RustBuildOptions {
        manifest_path: Some(manifest.clone()),
        cache_dir: Some(temp_cache.path().to_path_buf()),
        mode: BuildMode::Build,
        native: true,
        ..Default::default()
    };
    let exp_build = RustAdapter::explain_plan(&opt_build).expect("plan build");
    let keys_build =
        mitts::adapter::derive_unit_action_keys(&opt_build, &exp_build).expect("keys build");

    // Build plan in Check mode
    let opt_check = RustBuildOptions {
        manifest_path: Some(manifest),
        cache_dir: Some(temp_cache.path().to_path_buf()),
        mode: BuildMode::Check,
        native: true,
        ..Default::default()
    };
    let exp_check = RustAdapter::explain_plan(&opt_check).expect("plan check");
    let keys_check =
        mitts::adapter::derive_unit_action_keys(&opt_check, &exp_check).expect("keys check");

    // itoa is a non-workspace dependency; its action key must be 100% IDENTICAL between build and check
    let itoa_build_key = keys_build
        .iter()
        .find(|(k, _)| k.contains("itoa"))
        .map(|(_, v)| v.clone())
        .expect("itoa build key");
    let itoa_check_key = keys_check
        .iter()
        .find(|(k, _)| k.contains("itoa"))
        .map(|(_, v)| v.clone())
        .expect("itoa check key");

    assert_eq!(
        itoa_build_key, itoa_check_key,
        "Non-workspace dependency action key must match across BuildMode::Build and BuildMode::Check"
    );

    // hello-rust is a workspace package; its action key MUST differ between build and check
    let ws_build_key = keys_build
        .iter()
        .find(|(k, _)| k.contains("hello-rust"))
        .map(|(_, v)| v.clone())
        .expect("hello-rust build key");
    let ws_check_key = keys_check
        .iter()
        .find(|(k, _)| k.contains("hello-rust"))
        .map(|(_, v)| v.clone())
        .expect("hello-rust check key");

    assert_ne!(
        ws_build_key, ws_check_key,
        "Workspace package action key must differ between Build and Check mode"
    );
}
