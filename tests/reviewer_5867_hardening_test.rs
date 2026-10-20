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
use mitts::adapter::rust::profile::resolve_unit_debuginfo;
use mitts::adapter::rust::target_filter::is_target_cfg_incompatible_with_target;
use mitts::adapter::rust::types::{PlannedCompilationUnit, RustAdapterError};
use mitts::adapter::rust::RustcInvocation;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use tempfile::tempdir;

static TEST_ENV_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn test_7a_fail_closed_on_broken_rustc() {
    let _lock = TEST_ENV_LOCK.lock().unwrap();
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

    assert!(res.is_err(), "Must fail closed when rustc invocation fails");
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
    let _lock = TEST_ENV_LOCK.lock().unwrap();
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
    assert_eq!(res_no_target, None, "TARGET env variable must be ignored");

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
fn test_5a_build_target_in_config_passes_target_to_rustc() {
    let dir = tempdir().unwrap();
    let cargo_dir = dir.path().join(".cargo");
    fs::create_dir_all(&cargo_dir).unwrap();
    fs::write(
        cargo_dir.join("config.toml"),
        "[build]\ntarget = \"x86_64-unknown-linux-musl\"\n",
    )
    .unwrap();

    let manifest_path = dir.path().join("Cargo.toml");
    let src_dir = dir.path().join("src");
    fs::create_dir_all(&src_dir).unwrap();
    let src_path = src_dir.join("lib.rs");
    fs::write(&src_path, "pub fn hello() {}\n").unwrap();
    fs::write(
        &manifest_path,
        "[package]\nname = \"test_pkg\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();

    let unit = PlannedCompilationUnit {
        package_id: "test_pkg@0.1.0".to_string(),
        package_name: "test_pkg".to_string(),
        target_name: "test_pkg".to_string(),
        kind: "lib".to_string(),
        manifest_path: manifest_path.clone(),
        src_path: src_path.clone(),
        workspace_root: Some(dir.path().to_path_buf()),
        for_host: false,
        ..Default::default()
    };

    let out_dir = dir.path().join("target/out");
    let inv = RustcInvocation {
        unit: &unit,
        action_key: "test_action_key_1234567890",
        out_dir: &out_dir,
        dest_binary: None,
        extern_crates: &[],
        release: false,
        features: &[],
        directives: None,
        mode: mitts::adapter::BuildMode::Build,
        extra_compiler_args: &[],
        jobserver_auth: None,
        deps_dir: None,
        target_dir: None,
        cache_root: None,
    };

    let (cmd, _, _) = mitts::adapter::rust::compiler_args::build_compiler_command(&inv).unwrap();
    let args: Vec<String> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().to_string())
        .collect();

    let target_idx = args.iter().position(|a| a == "--target");
    assert!(
        target_idx.is_some(),
        "Rustc command must contain --target when build.target is set in config: args = {:?}",
        args
    );
    assert_eq!(
        args.get(target_idx.unwrap() + 1).map(|s| s.as_str()),
        Some("x86_64-unknown-linux-musl"),
        "Target value must match build.target from config"
    );

    // Host unit (for_host = true) must NOT receive --target
    let host_unit = PlannedCompilationUnit {
        for_host: true,
        kind: "proc-macro".to_string(),
        ..unit.clone()
    };
    let host_inv = RustcInvocation {
        unit: &host_unit,
        ..inv
    };
    let (host_cmd, _, _) =
        mitts::adapter::rust::compiler_args::build_compiler_command(&host_inv).unwrap();
    let host_args: Vec<String> = host_cmd
        .get_args()
        .map(|a| a.to_string_lossy().to_string())
        .collect();
    assert!(
        !host_args.iter().any(|a| a == "--target"),
        "Host unit must NOT receive --target from config: host_args = {:?}",
        host_args
    );
}

#[test]
fn test_5c_profile_precedence_and_deep_merging() {
    let ws_dir = tempdir().unwrap();
    let ws_manifest = ws_dir.path().join("Cargo.toml");
    let ws_toml = r#"
        [workspace]
        members = ["app"]

        [profile.dev]
        debug = 1
        opt-level = 0

        [profile.dev.package.syn]
        debug = 2

        [profile.dev.build-override]
        debug = 0
    "#;
    fs::write(&ws_manifest, ws_toml).unwrap();

    let cargo_dir = ws_dir.path().join(".cargo");
    fs::create_dir_all(&cargo_dir).unwrap();
    fs::write(
        cargo_dir.join("config.toml"),
        "[profile.dev]\nopt-level = 1\n",
    )
    .unwrap();

    let app_dir = ws_dir.path().join("app");
    fs::create_dir_all(&app_dir).unwrap();
    let app_manifest = app_dir.join("Cargo.toml");
    fs::write(
        &app_manifest,
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();

    // 1. Host unit of 'syn' must match [profile.dev.package.syn].debug = 2 (overriding build-override!)
    let dbg_syn_host = resolve_unit_debuginfo(
        &app_manifest,
        "syn",
        false,
        true, // for_host
        false,
        Some(ws_dir.path()),
    )
    .unwrap();
    assert_eq!(
        dbg_syn_host, 2,
        "package.syn debug = 2 must take precedence over build-override for host unit"
    );

    // 2. Generic host unit matches build-override: debug = 0
    let dbg_gen_host = resolve_unit_debuginfo(
        &app_manifest,
        "generic_macro",
        false,
        true, // for_host
        false,
        Some(ws_dir.path()),
    )
    .unwrap();
    assert_eq!(
        dbg_gen_host, 0,
        "generic host unit without package override must match build-override"
    );

    // 3. Merged top-level profile dev still has debug = 1 (not lost by config opt-level = 1)
    let dbg_app = resolve_unit_debuginfo(
        &app_manifest,
        "app",
        true, // is_workspace
        false,
        false,
        Some(ws_dir.path()),
    )
    .unwrap();
    assert_eq!(
        dbg_app, 1,
        "Workspace member must retain debug = 1 merged from Cargo.toml"
    );
}

#[test]
fn test_5d_b37f4e0_regression_host_vs_target_extern_crates() {
    use mitts::adapter::rust::dag_unit_externs::{
        resolve_extern_crates, resolve_extern_crates_for_build_script,
    };

    let dir = tempdir().unwrap();
    let manifest_path = dir.path().join("Cargo.toml");
    let toml_str = r#"
        [package]
        name = "dual_dep_pkg"
        version = "0.1.0"
        edition = "2021"

        [dependencies]
        shared_dep = "1.0"

        [build-dependencies]
        shared_dep = "1.0"
    "#;
    fs::write(&manifest_path, toml_str).unwrap();

    let unit = PlannedCompilationUnit {
        package_id: "dual_dep_pkg@0.1.0".to_string(),
        package_name: "dual_dep_pkg".to_string(),
        target_name: "dual_dep_pkg".to_string(),
        manifest_path: manifest_path.clone(),
        dependencies: vec![
            "shared_dep@1.0.0::lib:shared_dep".to_string(),
            "shared_dep@1.0.0::lib:shared_dep:host".to_string(),
        ],
        for_host: false,
        ..Default::default()
    };

    let host_artifact = PathBuf::from("/artifacts/host/libshared_dep.rlib");
    let target_artifact = PathBuf::from("/artifacts/target/libshared_dep.rlib");

    let mut completed = HashMap::new();
    completed.insert(
        "shared_dep@1.0.0::lib:shared_dep".to_string(),
        target_artifact.clone(),
    );
    completed.insert(
        "shared_dep@1.0.0::lib:shared_dep:host".to_string(),
        host_artifact.clone(),
    );

    // 1. Build script (always for host) must resolve the host artifact (:host)
    let build_externs = resolve_extern_crates_for_build_script(&unit, &completed);
    assert_eq!(
        build_externs,
        vec![("shared_dep".to_string(), host_artifact.clone())],
        "Build script must receive :host artifact when both copies exist"
    );

    // 2. Normal target unit (for_host = false) must resolve the target artifact
    let target_externs = resolve_extern_crates(&unit, &completed);
    assert_eq!(
        target_externs,
        vec![("shared_dep".to_string(), target_artifact)],
        "Target unit must receive target artifact when both copies exist"
    );
}
