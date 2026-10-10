/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::rust::key_flags::{
    receives_target_rustflags, resolve_explicit_target_from_args_and_dir,
};
use mitts::adapter::rust::manifest::CargoToml;
use mitts::adapter::rust::profile::resolve_unit_debuginfo;
use mitts::adapter::rust::target_filter::is_target_cfg_incompatible_with_target;
use mitts::adapter::rust::types::{PlannedCompilationUnit, RustAdapterError};
use std::fs;
use tempfile::tempdir;

#[test]
fn test_7a_fail_closed_on_broken_rustc() {
    let dir = tempdir().unwrap();
    let old_rustc = std::env::var_os("RUSTC");
    std::env::set_var("RUSTC", "/nonexistent_rustc_binary_for_test");

    let res = is_target_cfg_incompatible_with_target(
        "cfg(target_os = \"linux\")",
        false,
        None,
        dir.path(),
    );

    if let Some(ref val) = old_rustc {
        std::env::set_var("RUSTC", val);
    } else {
        std::env::remove_var("RUSTC");
    }

    assert!(
        res.is_err(),
        "Must fail closed when rustc invocation fails"
    );
    match res.unwrap_err() {
        RustAdapterError::MetadataFailed(msg) => {
            assert!(
                msg.contains("Failed to execute") || msg.contains("nonexistent_rustc_binary"),
                "Error message should explain failure: {}",
                msg
            );
        }
        other => panic!("Expected MetadataFailed, got {:?}", other),
    }
}

#[test]
fn test_7b_target_resolution_precedence_and_no_target_env() {
    let dir = tempdir().unwrap();
    let cargo_dir = dir.path().join(".cargo");
    fs::create_dir_all(&cargo_dir).unwrap();
    fs::write(
        cargo_dir.join("config.toml"),
        "[build]\ntarget = \"x86_64-unknown-linux-musl\"\n",
    )
    .unwrap();

    let old_target = std::env::var_os("TARGET");
    let old_cargo_build_target = std::env::var_os("CARGO_BUILD_TARGET");

    // 1. Ambient TARGET env variable must NOT be read (reviewer 7b)
    std::env::set_var("TARGET", "armv7-unknown-linux-gnueabihf");
    std::env::remove_var("CARGO_BUILD_TARGET");

    let empty_dir = tempdir().unwrap();
    let res_no_target = resolve_explicit_target_from_args_and_dir(&[], empty_dir.path());
    assert_eq!(
        res_no_target, None,
        "TARGET env variable must be ignored"
    );

    // 2. build.target from config.toml is loaded
    let res_cfg = resolve_explicit_target_from_args_and_dir(&[], dir.path());
    assert_eq!(
        res_cfg.as_deref(),
        Some("x86_64-unknown-linux-musl"),
        "Must read build.target from Cargo config"
    );

    // 3. CARGO_BUILD_TARGET env overrides config.toml
    std::env::set_var("CARGO_BUILD_TARGET", "aarch64-unknown-linux-gnu");
    let res_env = resolve_explicit_target_from_args_and_dir(&[], dir.path());
    assert_eq!(
        res_env.as_deref(),
        Some("aarch64-unknown-linux-gnu"),
        "CARGO_BUILD_TARGET must take precedence over config"
    );

    // 4. CLI --target overrides CARGO_BUILD_TARGET and config.toml
    let args = vec![
        "--target".to_string(),
        "riscv64gc-unknown-linux-gnu".to_string(),
    ];
    let res_cli = resolve_explicit_target_from_args_and_dir(&args, dir.path());
    assert_eq!(
        res_cli.as_deref(),
        Some("riscv64gc-unknown-linux-gnu"),
        "CLI --target must take highest precedence"
    );

    if let Some(ref val) = old_target {
        std::env::set_var("TARGET", val);
    } else {
        std::env::remove_var("TARGET");
    }
    if let Some(ref val) = old_cargo_build_target {
        std::env::set_var("CARGO_BUILD_TARGET", val);
    } else {
        std::env::remove_var("CARGO_BUILD_TARGET");
    }
}

#[test]
fn test_7c_rustflags_host_isolation_under_cross_compilation() {
    let host_unit = PlannedCompilationUnit {
        kind: "proc-macro".to_string(),
        for_host: true,
        ..Default::default()
    };
    let target_unit = PlannedCompilationUnit {
        kind: "lib".to_string(),
        for_host: false,
        ..Default::default()
    };

    // When compiling natively (None explicit target), all units receive rustflags
    assert!(receives_target_rustflags(&host_unit, None));
    assert!(receives_target_rustflags(&target_unit, None));

    // When cross-compiling (Some explicit target), host unit must NOT receive target rustflags
    let cross_target = Some("x86_64-unknown-linux-musl");
    assert!(!receives_target_rustflags(&host_unit, cross_target));
    assert!(receives_target_rustflags(&target_unit, cross_target));
}

#[test]
fn test_7d_profile_debuginfo_package_and_wildcard_overrides() {
    let ws_dir = tempdir().unwrap();
    let ws_manifest = ws_dir.path().join("Cargo.toml");
    let ws_toml = r#"
        [workspace]
        members = ["app"]

        [profile.dev]
        debug = 2

        [profile.dev.package."*"]
        debug = 1

        [profile.dev.package.special_dep]
        debug = 0

        [profile.dev.build-override]
        debug = 1
    "#;
    fs::write(&ws_manifest, ws_toml).unwrap();

    let app_dir = ws_dir.path().join("app");
    fs::create_dir_all(&app_dir).unwrap();
    let app_manifest = app_dir.join("Cargo.toml");
    fs::write(
        &app_manifest,
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();

    // Dependency located in isolated directory with its own irrelevant [profile]
    let dep_dir = tempdir().unwrap();
    let dep_manifest = dep_dir.path().join("Cargo.toml");
    fs::write(
        &dep_manifest,
        "[package]\nname = \"special_dep\"\nversion = \"0.1.0\"\n[profile.dev]\ndebug = 2\n",
    )
    .unwrap();

    // 1. Dependency with specific override: special_dep -> 0
    let dbg_special = resolve_unit_debuginfo(
        &dep_manifest,
        "special_dep",
        false,
        false,
        false,
        Some(ws_dir.path()),
    );
    assert_eq!(
        dbg_special, 0,
        "special_dep must match [profile.dev.package.special_dep].debug = 0"
    );

    // 2. Normal dependency matches wildcard "*": generic_dep -> 1
    let dbg_wildcard = resolve_unit_debuginfo(
        &dep_manifest,
        "generic_dep",
        false,
        false,
        false,
        Some(ws_dir.path()),
    );
    assert_eq!(
        dbg_wildcard, 1,
        "generic_dep must match [profile.dev.package.'*'].debug = 1"
    );

    // 3. Host unit matches build-override: host -> 1
    let dbg_host = resolve_unit_debuginfo(
        &app_manifest,
        "app",
        true,
        true,
        false,
        Some(ws_dir.path()),
    );
    assert_eq!(
        dbg_host, 1,
        "host unit must match [profile.dev.build-override].debug = 1"
    );
}

#[test]
fn test_7e_b37f4e0_regression_host_deps_for_build_script() {
    let dir = tempdir().unwrap();
    let manifest_path = dir.path().join("Cargo.toml");
    let toml_str = r#"
        [package]
        name = "pkg_with_build"
        version = "0.1.0"
        edition = "2021"

        [dependencies]
        common = "1.0"

        [build-dependencies]
        codegen_helper = "2.0"
    "#;
    fs::write(&manifest_path, toml_str).unwrap();
    let toml = CargoToml::from_file(&manifest_path).unwrap();

    // Target unit should have common but NOT build-dep codegen_helper
    let target_has_common = mitts::adapter::rust::dependency_check::has_active_dependency(
        &toml,
        "common",
        &[],
        false,
        false,
        None,
        dir.path(),
    )
    .unwrap();
    assert!(target_has_common);

    let target_has_codegen = mitts::adapter::rust::dependency_check::has_active_dependency(
        &toml,
        "codegen_helper",
        &[],
        false,
        false,
        None,
        dir.path(),
    )
    .unwrap();
    assert!(
        !target_has_codegen,
        "Target unit without build script must not have build dependencies"
    );

    // Build script unit (has_build_script = true) DOES activate codegen_helper
    let build_has_codegen = mitts::adapter::rust::dependency_check::has_active_dependency(
        &toml,
        "codegen_helper",
        &[],
        false,
        true,
        None,
        dir.path(),
    )
    .unwrap();
    assert!(
        build_has_codegen,
        "Build script unit must activate build dependencies"
    );
}
