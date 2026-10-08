/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::rust::manifest::CargoToml;
use mitts::core::build_lock::{
    FastBuildLock, FreshnessCheckArgs, FreshnessResult, RecordStateArgs,
};
use mitts::core::source_store::SourceStore;
use std::fs;
use std::sync::Mutex;
use tempfile::TempDir;

static ENV_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn test_m8_build_lock_missing_lock_and_manifest() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    let manifest = root.join("Cargo.toml");
    let lock_file = root.join("Cargo.lock");
    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir).unwrap();
    let lib_rs = src_dir.join("lib.rs");

    fs::write(
        &manifest,
        "[package]\nname = \"fresh_test\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(&lock_file, "# lockfile\n").unwrap();
    fs::write(&lib_rs, "pub fn f() {}\n").unwrap();

    let record_args = RecordStateArgs {
        project_dir: root,
        profile: "dev",
        package: &None,
        features: &[],
        no_default_features: false,
        all_features: false,
        toolchain_version: "rustc 1.88.0",
        action_count: 1,
        root_binary: None,
        outputs: &[],
        stderr: None,
    };

    // record_state writes .mitts/build-dev.lock automatically
    FastBuildLock::record_state(&record_args).unwrap();

    let check_args = FreshnessCheckArgs {
        project_dir: root,
        profile: "dev",
        package: &None,
        features: &[],
        no_default_features: false,
        all_features: false,
        toolchain_version: "rustc 1.88.0",
    };

    // 1. Delete Cargo.lock -> "Cargo.lock missing"
    fs::remove_file(&lock_file).unwrap();
    match FastBuildLock::check_freshness(&check_args) {
        FreshnessResult::Stale(reason) => assert!(reason.contains("Cargo.lock missing")),
        _ => panic!("Expected Cargo.lock missing stale result"),
    }

    // Restore Cargo.lock, then delete Cargo.toml -> "Manifest missing"
    fs::write(&lock_file, "# lockfile\n").unwrap();
    fs::remove_file(&manifest).unwrap();
    match FastBuildLock::check_freshness(&check_args) {
        FreshnessResult::Stale(reason) => assert!(reason.contains("Manifest missing")),
        _ => panic!("Expected Manifest missing stale result"),
    }
}

#[test]
fn test_m8_manifest_autobins_and_exact_workspace_members() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
    fs::create_dir_all(root.join("tests")).unwrap();
    fs::write(root.join("tests/integration_test.rs"), "fn main() {}\n").unwrap();

    // 1. [[bin]] with path = None and src/bin/<name>.rs missing -> defaults to src/main.rs
    let toml_str = r#"
[package]
name = "autobin_pkg"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "autobin_exec"

[[test]]
name = "integration_test"
"#;
    let toml: CargoToml = toml::from_str(toml_str).unwrap();
    let targets = toml.discover_targets(root);
    assert!(targets
        .iter()
        .any(|t| t.name == "autobin_exec" && t.src_path.ends_with("src/main.rs")));
    let test_targets = toml.discover_test_targets(root);
    assert!(
        test_targets
            .iter()
            .any(|t| t.name == "integration_test"
                && t.src_path.ends_with("tests/integration_test.rs"))
    );

    // 2. Exact workspace member path without glob
    let ws_toml_str = r#"
[workspace]
members = ["member_a"]
"#;
    let ws_toml: CargoToml = toml::from_str(ws_toml_str).unwrap();
    let member_dir = root.join("member_a");
    fs::create_dir_all(&member_dir).unwrap();
    fs::write(
        member_dir.join("Cargo.toml"),
        "[package]\nname = \"member_a\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();

    let members = ws_toml.discover_workspace_members(root);
    assert_eq!(members.len(), 1);
    assert_eq!(members[0].0, "member_a");
}

#[test]
fn test_m8_source_store_registry_empty_index_and_overwrite() {
    let _lock = ENV_LOCK.lock().unwrap();
    let temp = TempDir::new().unwrap();
    let home = temp.path();
    let old_home = std::env::var_os("HOME");
    std::env::set_var("HOME", home);

    // 1. ~/.cargo/registry/src exists but is empty -> discover_cargo_registry_src creates default dir
    let reg_src = home.join(".cargo").join("registry").join("src");
    fs::create_dir_all(&reg_src).unwrap();
    let discovered_src = SourceStore::discover_cargo_registry_src();
    assert!(discovered_src.is_some());
    assert!(discovered_src
        .unwrap()
        .ends_with("index.crates.io-1949cf8c6b5b557f"));

    // 2. ~/.cargo/registry/cache exists but is empty -> discover_cargo_registry_cache creates default dir
    let reg_cache = home.join(".cargo").join("registry").join("cache");
    fs::create_dir_all(&reg_cache).unwrap();
    let discovered_cache = SourceStore::discover_cargo_registry_cache();
    assert!(discovered_cache.is_some());
    assert!(discovered_cache
        .unwrap()
        .ends_with("index.crates.io-1949cf8c6b5b557f"));

    if let Some(h) = old_home {
        std::env::set_var("HOME", h);
    }
}
