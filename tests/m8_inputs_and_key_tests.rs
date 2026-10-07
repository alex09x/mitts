/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::rust::inputs::{append_unit_dep_info_inputs, hash_package_base_inputs, hash_unit_inputs};
use mitts::adapter::rust::key::{resolve_effective_target_triple, resolve_unit_pkg_dir};
use mitts::adapter::rust::types::{PlannedCompilationUnit, RustBuildOptions};
use mitts::core::action::ToolchainInfo;
use std::fs;

#[test]
fn test_m8_inputs_immutable_registry_and_missing_cargo_toml() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let pkg_dir = temp_dir.path().join("my-crate-1.0.0");
    fs::create_dir_all(&pkg_dir).unwrap();

    // 1. Missing Cargo.toml
    let err = hash_package_base_inputs(&pkg_dir, None, false);
    assert!(err.is_err(), "Expected missing Cargo.toml error");

    // 2. Immutable fast-path with .cargo-ok
    fs::write(pkg_dir.join(".cargo-ok"), "ok").unwrap();
    fs::write(pkg_dir.join("Cargo.toml"), "[package]\nname=\"my-crate\"\nversion=\"1.0.0\"\n").unwrap();
    let inputs = hash_package_base_inputs(&pkg_dir, None, false).expect("hash immutable pkg");
    assert_eq!(inputs.len(), 2);
    assert_eq!(inputs[0].0, "Cargo.toml");
    assert_eq!(inputs[1].0, "immutable_pkg");
}

#[test]
fn test_m8_inputs_workspace_manifests_and_configs() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let root = temp_dir.path();

    let ws_manifest = root.join("Cargo.toml");
    fs::write(&ws_manifest, "[workspace]\nmembers = [\"crates/*\"]\n").unwrap();

    let ws_lock = root.join("Cargo.lock");
    fs::write(&ws_lock, "# lockfile\n").unwrap();

    let ws_cargo = root.join(".cargo");
    fs::create_dir_all(&ws_cargo).unwrap();
    fs::write(ws_cargo.join("config.toml"), "[build]\njobs = 4\n").unwrap();
    fs::write(ws_cargo.join("config"), "# legacy config\n").unwrap();

    let crate_dir = root.join("crates").join("lib-a");
    let crate_src = crate_dir.join("src");
    fs::create_dir_all(&crate_src).unwrap();
    fs::write(crate_dir.join("Cargo.toml"), "[package]\nname=\"lib-a\"\nversion=\"0.1.0\"\n").unwrap();
    fs::write(crate_src.join("lib.rs"), "pub fn a() {}\n").unwrap();

    // Directory that should be ignored by broad scan
    let target_dir = crate_dir.join("target");
    fs::create_dir_all(&target_dir).unwrap();
    fs::write(target_dir.join("artifact.bin"), "build artifact").unwrap();

    // Package asset directory like test_data/ must be included (P2 cache correctness)
    let test_data_dir = crate_dir.join("test_data");
    fs::create_dir_all(&test_data_dir).unwrap();
    fs::write(test_data_dir.join("dump.bin"), "large data").unwrap();

    let inputs = hash_package_base_inputs(&crate_dir, Some(&ws_manifest), true)
        .expect("hash workspace crate inputs");

    let keys: Vec<&str> = inputs.iter().map(|(k, _)| k.as_str()).collect();
    assert!(keys.contains(&"Cargo.toml"));
    assert!(keys.contains(&"Cargo.lock"));
    assert!(keys.contains(&"workspace:Cargo.toml"));
    assert!(keys.contains(&"workspace:.cargo/config.toml"));
    assert!(keys.contains(&"workspace:.cargo/config"));
    assert!(keys.contains(&"src/lib.rs"));
    assert!(keys.contains(&"test_data/dump.bin"), "Asset files in test_data/ must be included for cache correctness");
    assert!(!keys.contains(&"target/artifact.bin"), "target/ directory must be skipped");
}

#[test]
fn test_m8_inputs_dep_info_supplement() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let pkg_dir = temp_dir.path().join("pkg");
    let target_dir = temp_dir.path().join("target");
    let debug_dir = target_dir.join("debug");
    fs::create_dir_all(&pkg_dir).unwrap();
    fs::create_dir_all(&debug_dir).unwrap();

    fs::write(pkg_dir.join("Cargo.toml"), "[package]\nname=\"pkg\"\nversion=\"0.1.0\"\n").unwrap();
    let lib_rs = pkg_dir.join("lib.rs");
    fs::write(&lib_rs, "fn main() {}\n").unwrap();

    // Asset outside of standard package or filtered
    let asset_file = temp_dir.path().join("data.bin");
    fs::write(&asset_file, "binary payload").unwrap();

    // Synthetic dep-info file in target/debug
    let dep_info = debug_dir.join("pkg.d");
    fs::write(
        &dep_info,
        format!(
            "pkg.rlib: {} {}\n",
            lib_rs.display(),
            asset_file.display()
        ),
    ).unwrap();

    let base_inputs = vec![("lib.rs".to_string(), "hash_lib".to_string())];
    let supplemented = append_unit_dep_info_inputs(base_inputs, &pkg_dir, &target_dir, "pkg")
        .expect("append dep info");

    let keys: Vec<&str> = supplemented.iter().map(|(k, _)| k.as_str()).collect();
    // lib.rs is inside package and was already in base_inputs, so not duplicated
    assert_eq!(keys.iter().filter(|&&k| k == "lib.rs").count(), 1);
    // asset_file was outside package_dir and added
    assert!(keys.iter().any(|k| k.contains("data.bin")));

    // Test hash_unit_inputs convenience wrapper
    let unit_inputs = hash_unit_inputs(&pkg_dir, None, &target_dir, "pkg", false)
        .expect("hash unit inputs");
    assert!(!unit_inputs.is_empty());
}

#[test]
fn test_m8_key_target_triple_and_pkg_dir() {
    let toolchain = ToolchainInfo {
        name: "rustc".to_string(),
        version: "rustc 1.80.0".to_string(),
        target_triple: Some("x86_64-apple-darwin".to_string()),
    };

    // 1. Toolchain default
    let opt_default = RustBuildOptions::default();
    let t1 = resolve_effective_target_triple(&opt_default, &toolchain);
    assert_eq!(t1, "x86_64-apple-darwin");

    // 2. CLI --target flag with separate value
    let opt_cli_flag = RustBuildOptions {
        extra_args: vec!["--target".to_string(), "aarch64-unknown-linux-gnu".to_string()],
        ..RustBuildOptions::default()
    };
    let t2 = resolve_effective_target_triple(&opt_cli_flag, &toolchain);
    assert_eq!(t2, "aarch64-unknown-linux-gnu");

    // 3. CLI --target=val format
    let opt_cli_eq = RustBuildOptions {
        extra_args: vec!["--target=wasm32-unknown-unknown".to_string()],
        ..RustBuildOptions::default()
    };
    let t3 = resolve_effective_target_triple(&opt_cli_eq, &toolchain);
    assert_eq!(t3, "wasm32-unknown-unknown");

    // 4. resolve_unit_pkg_dir
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let manifest = temp_dir.path().join("Cargo.toml");
    fs::write(&manifest, "[package]\nname=\"foo\"\nversion=\"0.1.0\"\n").unwrap();
    let unit = PlannedCompilationUnit {
        package_id: "foo 0.1.0".to_string(),
        package_name: "foo".to_string(),
        version: "0.1.0".to_string(),
        target_name: "foo".to_string(),
        kind: "lib".to_string(),
        src_path: temp_dir.path().join("src").join("lib.rs"),
        manifest_path: manifest.clone(),
        cache_eligibility: "eligible".to_string(),
        is_workspace: true,
        env_vars: vec![],
        dependencies: vec![],
        edition: Some("2021".to_string()),
        features: vec![],
    };
    let resolved_dir = resolve_unit_pkg_dir(&unit);
    assert_eq!(resolved_dir, temp_dir.path());
}
