/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::rust::key::{compute_unit_flags, derive_unit_action_keys, parallel_map_chunks};
use mitts::adapter::rust::types::{
    BuildMode, EffectivePlanExplanation, PlannedCompilationUnit, RustBuildOptions,
};
use mitts::core::action::ToolchainInfo;
use std::fs;
use tempfile::TempDir;

#[test]
fn test_m8_parallel_map_chunks_edge_cases() {
    // 1. Empty slice
    let empty: Vec<i32> = Vec::new();
    let res: Vec<i32> = parallel_map_chunks(&empty, |x| *x * 2);
    assert!(res.is_empty());

    // 2. Single item (num_threads <= 1 branch)
    let single = vec![42];
    let res: Vec<i32> = parallel_map_chunks(&single, |x| *x + 1);
    assert_eq!(res, vec![43]);

    // 3. Multi items
    let items: Vec<i32> = (0..50).collect();
    let res: Vec<i32> = parallel_map_chunks(&items, |x| *x * 3);
    assert_eq!(res.len(), 50);
    assert_eq!(res[10], 30);

    // 4. Panic propagation via resume_unwind
    let panic_res = std::panic::catch_unwind(|| {
        parallel_map_chunks(&[1, 2, 3], |x| {
            if *x == 2 {
                panic!("intentional worker error");
            }
            *x
        });
    });
    assert!(panic_res.is_err());
}

#[test]
fn test_m8_compute_unit_flags_comprehensive_env_macros() {
    let tmp = TempDir::new().unwrap();
    let pkg_dir = tmp.path().to_path_buf();
    std::env::set_var("MITTS_TEST_DEFINED_VAR", "active_val");

    let unit_bin = PlannedCompilationUnit {
        package_id: "mitts-test-app@1.2.3".to_string(),
        package_name: "mitts-test-app".to_string(),
        version: "1.2.3".to_string(),
        target_name: "mitts_test_app".to_string(),
        kind: "bin".to_string(),
        src_path: pkg_dir.join("src").join("main.rs"),
        manifest_path: pkg_dir.join("Cargo.toml"),
        dependencies: Vec::new(),
        features: Vec::new(),
        env_vars: vec![
            "CARGO_MANIFEST_DIR".to_string(),
            "CARGO_PKG_NAME".to_string(),
            "CARGO_PKG_VERSION".to_string(),
            "CARGO_PKG_VERSION_MAJOR".to_string(),
            "CARGO_PKG_VERSION_MINOR".to_string(),
            "CARGO_PKG_VERSION_PATCH".to_string(),
            "CARGO_CRATE_NAME".to_string(),
            "CARGO_BIN_NAME".to_string(),
            "MITTS_TEST_DEFINED_VAR".to_string(),
            "MITTS_TEST_UNSET_VAR".to_string(),
        ],
        cache_eligibility: "eligible".to_string(),
        is_workspace: true,
        is_primary: false,
        edition: Some("2021".to_string()),
        for_host: false,
        workspace_root: None,
    };

    let flags = compute_unit_flags(&unit_bin, &pkg_dir, &[], BuildMode::Build);
    assert!(flags.iter().any(|f| f.contains("CARGO_MANIFEST_DIR=")));
    assert!(flags.iter().any(|f| f.contains("MODE=Build")));
    assert!(flags
        .iter()
        .any(|f| f.contains("CARGO_PKG_NAME=mitts-test-app")));
    assert!(flags
        .iter()
        .any(|f| f.contains("CARGO_BIN_NAME=mitts_test_app")));
    assert!(flags
        .iter()
        .any(|f| f.contains("ENV_MACRO:CARGO_PKG_VERSION_MAJOR=1")));
    assert!(flags
        .iter()
        .any(|f| f.contains("ENV_MACRO:CARGO_PKG_VERSION_MINOR=2")));
    assert!(flags
        .iter()
        .any(|f| f.contains("ENV_MACRO:CARGO_PKG_VERSION_PATCH=3")));
    assert!(flags
        .iter()
        .any(|f| f.contains("ENV_MACRO:MITTS_TEST_DEFINED_VAR=active_val")));
    assert!(flags
        .iter()
        .any(|f| f.contains("ENV_MACRO:MITTS_TEST_UNSET_VAR=<unset>")));

    // Test non-bin unit CARGO_BIN_NAME=<unset>
    let mut unit_lib = unit_bin.clone();
    unit_lib.kind = "lib".to_string();
    let flags_lib = compute_unit_flags(&unit_lib, &pkg_dir, &[], BuildMode::Build);
    assert!(flags_lib
        .iter()
        .any(|f| f.contains("ENV_MACRO:CARGO_BIN_NAME=<unset>")));
}

#[test]
fn test_m8_derive_unit_action_keys_cycle_fallback() {
    let tmp = TempDir::new().unwrap();
    let pkg_dir = tmp.path().to_path_buf();
    let cargo_toml = pkg_dir.join("Cargo.toml");
    fs::write(
        &cargo_toml,
        "[package]\nname = \"cycle-test\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    let src_dir = pkg_dir.join("src");
    fs::create_dir_all(&src_dir).unwrap();
    fs::write(src_dir.join("lib.rs"), "pub fn cycle() {}").unwrap();

    let unit_a = PlannedCompilationUnit {
        package_id: "cycle-a@0.1.0".to_string(),
        package_name: "cycle-a".to_string(),
        version: "0.1.0".to_string(),
        target_name: "cycle_a".to_string(),
        kind: "lib".to_string(),
        src_path: src_dir.join("lib.rs"),
        manifest_path: cargo_toml.clone(),
        dependencies: vec!["cycle-b@0.1.0::lib:cycle_b".to_string()],
        features: Vec::new(),
        env_vars: Vec::new(),
        cache_eligibility: "eligible".to_string(),
        is_workspace: true,
        is_primary: false,
        edition: Some("2021".to_string()),
        for_host: false,
        workspace_root: None,
    };

    let unit_b = PlannedCompilationUnit {
        package_id: "cycle-b@0.1.0".to_string(),
        package_name: "cycle-b".to_string(),
        version: "0.1.0".to_string(),
        target_name: "cycle_b".to_string(),
        kind: "lib".to_string(),
        src_path: src_dir.join("lib.rs"),
        manifest_path: cargo_toml.clone(),
        dependencies: vec!["cycle-a@0.1.0::lib:cycle_a".to_string()],
        features: Vec::new(),
        env_vars: Vec::new(),
        cache_eligibility: "eligible".to_string(),
        is_workspace: true,
        is_primary: false,
        edition: Some("2021".to_string()),
        for_host: false,
        workspace_root: None,
    };

    let explanation = EffectivePlanExplanation {
        manifest_path: cargo_toml,
        target_directory: tmp.path().join("target"),
        profile: "dev".to_string(),
        toolchain: ToolchainInfo {
            name: "rustc".to_string(),
            version: "rustc 1.85.0".to_string(),
            target_triple: Some("x86_64-unknown-linux-gnu".to_string()),
        },
        units: vec![unit_a, unit_b],
    };

    let opt = RustBuildOptions::default();
    let keys = derive_unit_action_keys(&opt, &explanation).unwrap();
    assert_eq!(keys.len(), 2);
    assert!(keys.contains_key("cycle-a@0.1.0::lib:cycle_a"));
    assert!(keys.contains_key("cycle-b@0.1.0::lib:cycle_b"));
}
