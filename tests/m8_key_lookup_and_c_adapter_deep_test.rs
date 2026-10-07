/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::c::{CAdapter, CBuildOptions};
use mitts::adapter::rust::key::derive_unit_action_keys;
use mitts::adapter::rust::source_lookup::find_crate_source_dir;
use mitts::adapter::rust::types::{EffectivePlanExplanation, PlannedCompilationUnit, RustBuildOptions};
use mitts::core::action::ToolchainInfo;
use std::fs;

#[test]
fn test_m8_source_lookup_comprehensive_hierarchies() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    let fake_home = root.join("fake_home");
    let prev_home = std::env::var("HOME").ok();
    std::env::set_var("HOME", &fake_home);

    // 1. Nested git crate under crates/sub_crate
    let git_commit_dir = fake_home
        .join(".cargo")
        .join("git")
        .join("checkouts")
        .join("repo-xyz-12345")
        .join("abcdef12");
    let nested_crate_dir = git_commit_dir.join("crates").join("my_nested_git_crate");
    fs::create_dir_all(&nested_crate_dir).unwrap();
    fs::write(
        nested_crate_dir.join("Cargo.toml"),
        r#"[package]
name = "my_nested_git_crate"
version = "0.1.0"
"#,
    )
    .unwrap();

    let found_git = find_crate_source_dir(
        "my_nested_git_crate",
        "0.1.0",
        Some("git+https://github.com/foo/bar#abcdef1234567890"),
    );
    assert_eq!(found_git, Some(nested_crate_dir));

    // 2. Mitts cache sources (~/.cache/mitts/sources/name-version)
    let mitts_crate = fake_home
        .join(".cache")
        .join("mitts")
        .join("sources")
        .join("mitts_lib-1.2.0");
    fs::create_dir_all(&mitts_crate).unwrap();
    fs::write(
        mitts_crate.join("Cargo.toml"),
        r#"[package]
name = "mitts_lib"
version = "1.2.0"
"#,
    )
    .unwrap();

    let found_mitts = find_crate_source_dir("mitts_lib", "1.2.0", None);
    assert_eq!(found_mitts, Some(mitts_crate));

    // 3. Not found anywhere
    let not_found = find_crate_source_dir("missing_ghost_pkg", "9.9.9", None);
    assert!(not_found.is_none());

    if let Some(h) = prev_home {
        std::env::set_var("HOME", h);
    }
}

#[test]
fn test_m8_key_derivation_env_flags_and_cyclic_dependency() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    let manifest_path = root.join("Cargo.toml");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        &manifest_path,
        r#"[package]
name = "cyclic_a"
version = "0.1.0"
edition = "2021"
"#,
    )
    .unwrap();
    fs::write(root.join("src/lib.rs"), b"pub fn f() {}").unwrap();

    let toolchain = ToolchainInfo {
        name: "rustc".to_string(),
        version: "1.80.0".to_string(),
        target_triple: Some("x86_64-unknown-linux-gnu".to_string()),
    };

    // Set relevant env vars
    std::env::set_var("RUSTFLAGS", "-C target-cpu=native");
    std::env::set_var("RUSTC_WRAPPER", "sccache");
    std::env::set_var("TARGET", "x86_64-unknown-linux-gnu");

    // Construct two mutually dependent compilation units to exercise the unready/cyclic loop
    let unit_a = PlannedCompilationUnit {
        package_id: "cyclic_a@0.1.0".to_string(),
        package_name: "cyclic_a".to_string(),
        version: "0.1.0".to_string(),
        target_name: "cyclic_a".to_string(),
        kind: "lib".to_string(),
        src_path: root.join("src/lib.rs"),
        manifest_path: manifest_path.clone(),
        cache_eligibility: "Eligible".to_string(),
        is_workspace: true,
        env_vars: Vec::new(),
        dependencies: vec!["cyclic_b@0.1.0::lib:cyclic_b".to_string()],
        edition: Some("2021".to_string()),
        features: Vec::new(),
    };

    let unit_b = PlannedCompilationUnit {
        package_id: "cyclic_b@0.1.0".to_string(),
        package_name: "cyclic_b".to_string(),
        version: "0.1.0".to_string(),
        target_name: "cyclic_b".to_string(),
        kind: "lib".to_string(),
        src_path: root.join("src/lib.rs"),
        manifest_path: manifest_path.clone(),
        cache_eligibility: "Eligible".to_string(),
        is_workspace: true,
        env_vars: Vec::new(),
        dependencies: vec!["cyclic_a@0.1.0::lib:cyclic_a".to_string()],
        edition: Some("2021".to_string()),
        features: Vec::new(),
    };

    let plan = EffectivePlanExplanation {
        manifest_path,
        target_directory: root.join("target"),
        profile: "debug".to_string(),
        toolchain,
        units: vec![unit_a, unit_b],
    };

    let opt = RustBuildOptions::default();
    let keys = derive_unit_action_keys(&opt, &plan).unwrap();
    assert_eq!(keys.len(), 2);

    std::env::remove_var("RUSTFLAGS");
    std::env::remove_var("RUSTC_WRAPPER");
    std::env::remove_var("TARGET");
}

#[test]
fn test_m8_c_adapter_manifest_resolution_directory_and_errors() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    // 1. Directory containing Mitts.toml
    let project_dir = root.join("c_project");
    fs::create_dir_all(&project_dir).unwrap();
    fs::write(
        project_dir.join("Mitts.toml"),
        r#"[package]
name = "c_demo"
version = "1.0.0"
"#,
    )
    .unwrap();

    let opt_dir = CBuildOptions {
        manifest_path: Some(project_dir.clone()),
        ..Default::default()
    };
    let resolved = CAdapter::resolve_manifest(&opt_dir).unwrap();
    assert_eq!(resolved, project_dir.join("Mitts.toml"));

    // 2. parse_manifest error on missing name
    let bad_manifest = "[package]\nversion = \"1.0.0\"\n";
    assert!(CAdapter::parse_manifest(bad_manifest).is_err());

    // 3. parse_manifest without [bin] section
    let lib_only = "[package]\nname = \"clib\"\nversion = \"2.0.0\"\n";
    let parsed = CAdapter::parse_manifest(lib_only).unwrap();
    assert_eq!(parsed.package.name, "clib");
    assert!(parsed.bin.is_none());
}
