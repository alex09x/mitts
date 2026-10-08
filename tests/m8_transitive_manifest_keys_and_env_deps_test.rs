/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::{RustAdapter, RustBuildOptions};
use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

fn create_workspace_with_transitive_dep(root: &Path, asset_content: &str) {
    fs::create_dir_all(root.join("assets")).unwrap();
    fs::create_dir_all(root.join("crates/lib_a/src")).unwrap();
    fs::create_dir_all(root.join("crates/bin_b/src")).unwrap();

    fs::write(
        root.join("Cargo.toml"),
        r#"[workspace]
members = ["crates/lib_a", "crates/bin_b"]
resolver = "2"
"#,
    )
    .unwrap();

    fs::write(root.join("assets/data.txt"), asset_content).unwrap();

    fs::write(
        root.join("crates/lib_a/Cargo.toml"),
        r#"[package]
name = "lib_a"
version = "0.1.0"
edition = "2021"
"#,
    )
    .unwrap();

    fs::write(
        root.join("crates/lib_a/src/lib.rs"),
        r#"pub const DATA: &str = include_str!("../../../assets/data.txt");
pub fn get_data() -> &'static str {
    DATA
}
"#,
    )
    .unwrap();

    fs::write(
        root.join("crates/bin_b/Cargo.toml"),
        r#"[package]
name = "bin_b"
version = "0.1.0"
edition = "2021"

[dependencies]
lib_a = { path = "../lib_a" }
"#,
    )
    .unwrap();

    fs::write(
        root.join("crates/bin_b/src/main.rs"),
        r#"fn main() {
    print!("TRANSITIVE:{}", lib_a::get_data());
}
"#,
    )
    .unwrap();
}

#[test]
fn test_m8_transitive_dependent_dynamic_keys_across_clones() {
    let cache_dir = TempDir::new().unwrap();

    // 1. Clone A with asset "V1"
    let clone_a = TempDir::new().unwrap();
    create_workspace_with_transitive_dep(clone_a.path(), "V1");

    let opt_a = RustBuildOptions {
        manifest_path: Some(clone_a.path().join("Cargo.toml")),
        cache_dir: Some(cache_dir.path().to_path_buf()),
        ..Default::default()
    };

    let (_, res_a) = RustAdapter::build_native(&opt_a).expect("Clone A build failed");
    assert_eq!(res_a.exit_code, 0);

    let bin_a = clone_a.path().join("target/mitts/debug/bin_b");
    assert!(bin_a.is_file(), "Binary in Clone A must exist");
    let out_a = Command::new(&bin_a).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out_a.stdout), "TRANSITIVE:V1");

    // 2. Clone B with asset "V2" (fresh clone, empty target/mitts)
    let clone_b = TempDir::new().unwrap();
    create_workspace_with_transitive_dep(clone_b.path(), "V2");

    let opt_b = RustBuildOptions {
        manifest_path: Some(clone_b.path().join("Cargo.toml")),
        cache_dir: Some(cache_dir.path().to_path_buf()),
        ..Default::default()
    };

    let (_, res_b) = RustAdapter::build_native(&opt_b).expect("Clone B build failed");
    assert_eq!(res_b.exit_code, 0);

    let bin_b = clone_b.path().join("target/mitts/debug/bin_b");
    assert!(bin_b.is_file(), "Binary in Clone B must exist");
    let out_b = Command::new(&bin_b).output().unwrap();
    // CRITICAL (P0): bin_b MUST print V2! It must NOT hit stale cache of bin_b from Clone A!
    assert_eq!(
        String::from_utf8_lossy(&out_b.stdout),
        "TRANSITIVE:V2",
        "bin_b in Clone B must print V2, verifying dependent received new resolved key of lib_a"
    );

    // 3. Clone C with asset "V1" (fresh clone, empty target/mitts) -> 100% cache hit on both units
    let clone_c = TempDir::new().unwrap();
    create_workspace_with_transitive_dep(clone_c.path(), "V1");

    let opt_c = RustBuildOptions {
        manifest_path: Some(clone_c.path().join("Cargo.toml")),
        cache_dir: Some(cache_dir.path().to_path_buf()),
        ..Default::default()
    };

    let (_, res_c) = RustAdapter::build_native(&opt_c).expect("Clone C build failed");
    assert_eq!(res_c.exit_code, 0);

    let bin_c = clone_c.path().join("target/mitts/debug/bin_b");
    assert!(bin_c.is_file(), "Binary in Clone C must exist");
    let out_c = Command::new(&bin_c).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out_c.stdout), "TRANSITIVE:V1");
}

#[test]
fn test_m8_env_dep_tracking_and_manifest_invalidation() {
    let cache_dir = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();

    fs::create_dir_all(root.path().join("src")).unwrap();
    fs::write(
        root.path().join("Cargo.toml"),
        r#"[package]
name = "env_app"
version = "0.1.0"
edition = "2021"
"#,
    )
    .unwrap();

    fs::write(
        root.path().join("src/main.rs"),
        r#"fn main() {
    let val = env!("TEST_ENV_TRACKED_VAR");
    print!("VAL:{}", val);
}
"#,
    )
    .unwrap();

    // 1. Build with TEST_ENV_TRACKED_VAR = "ALPHA"
    std::env::set_var("TEST_ENV_TRACKED_VAR", "ALPHA");
    let opt1 = RustBuildOptions {
        manifest_path: Some(root.path().join("Cargo.toml")),
        cache_dir: Some(cache_dir.path().to_path_buf()),
        ..Default::default()
    };
    let (_, res1) = RustAdapter::build_native(&opt1).expect("Build 1 failed");
    assert_eq!(res1.exit_code, 0);

    let bin1 = root.path().join("target/mitts/debug/env_app");
    let out1 = Command::new(&bin1).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out1.stdout), "VAL:ALPHA");

    // 2. Remove target/mitts, change env var to "BETA", build again -> must recompile and print BETA
    let _ = fs::remove_dir_all(root.path().join("target/mitts"));
    std::env::set_var("TEST_ENV_TRACKED_VAR", "BETA");
    let opt2 = RustBuildOptions {
        manifest_path: Some(root.path().join("Cargo.toml")),
        cache_dir: Some(cache_dir.path().to_path_buf()),
        ..Default::default()
    };
    let (_, res2) = RustAdapter::build_native(&opt2).expect("Build 2 failed");
    assert_eq!(res2.exit_code, 0);

    let bin2 = root.path().join("target/mitts/debug/env_app");
    let out2 = Command::new(&bin2).output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out2.stdout),
        "VAL:BETA",
        "Must print BETA after env var changed, verified by # env-dep in manifest"
    );

    // 3. Remove target/mitts, switch back to "ALPHA" -> cache hit on ALPHA manifest entry!
    let _ = fs::remove_dir_all(root.path().join("target/mitts"));
    std::env::set_var("TEST_ENV_TRACKED_VAR", "ALPHA");
    let opt3 = RustBuildOptions {
        manifest_path: Some(root.path().join("Cargo.toml")),
        cache_dir: Some(cache_dir.path().to_path_buf()),
        ..Default::default()
    };
    let (_, res3) = RustAdapter::build_native(&opt3).expect("Build 3 failed");
    assert_eq!(res3.exit_code, 0);

    let bin3 = root.path().join("target/mitts/debug/env_app");
    let out3 = Command::new(&bin3).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out3.stdout), "VAL:ALPHA");
}

#[test]
fn test_m8_fail_closed_unhashable_tracked_input() {
    let staging_dir = TempDir::new().unwrap();
    let ws_dir = TempDir::new().unwrap();

    // Create a mock .d file referencing a non-existent external file outside package
    let d_file = staging_dir.path().join("mock.d");
    let missing_file = ws_dir.path().join("missing_external.txt");
    fs::write(
        &d_file,
        format!("mock.o: {}\n", missing_file.to_string_lossy()),
    )
    .unwrap();

    let unit = mitts::adapter::PlannedCompilationUnit {
        package_id: "mock_pkg".to_string(),
        package_name: "mock_pkg".to_string(),
        version: "0.1.0".to_string(),
        target_name: "mock_pkg".to_string(),
        kind: "lib".to_string(),
        src_path: ws_dir.path().join("crates/mock/src/lib.rs"),
        manifest_path: ws_dir.path().join("crates/mock/Cargo.toml"),
        cache_eligibility: "eligible".to_string(),
        is_workspace: true,
        env_vars: Vec::new(),
        dependencies: Vec::new(),
        edition: None,
        features: Vec::new(),
    };

    // Attempting to extract tracked extra inputs must FAIL (fail-closed, not dropping file silently)
    let res = mitts::adapter::rust::dag_unit_publish::extract_unit_extra_inputs(
        &unit,
        staging_dir.path(),
        Some(ws_dir.path()),
        None,
        None,
    );
    assert!(
        res.is_err(),
        "Extracting extra inputs must fail when a tracked file cannot be hashed/read (P1 fail-closed)"
    );
}
