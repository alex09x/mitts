/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::rust::compiler::RustcInvocation;
use mitts::adapter::rust::inputs::{
    append_unit_dep_info_inputs, hash_package_base_inputs, hash_unit_inputs,
};
use mitts::adapter::rust::types::PlannedCompilationUnit;
use mitts::adapter::BuildMode;
use mitts::cli::diagnostic::render_compiler_diagnostics;
use std::fs;
use std::sync::Mutex;
use tempfile::TempDir;

static ENV_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn test_m8_render_compiler_diagnostics_with_source_span_and_metadata() {
    let temp = TempDir::new().unwrap();
    let src_dir = temp.path().join("src");
    fs::create_dir_all(&src_dir).unwrap();

    let lib_rs = src_dir.join("lib.rs");
    fs::write(
        &lib_rs,
        b"pub fn calculate() {\n    let val = 42;\n    println!(\"{}\", val);\n}\n",
    )
    .unwrap();

    // 1. Structured compiler output with real file location, command, and cwd
    let structured_stderr = format!(
        r#"rustc failed for unit 'demo_calc@1.2.3::lib:demo_calc' (exit code 1):
  command: "rustc" "--crate-name" "demo_calc"
  cwd: {}
error[E0425]: cannot find value `xyz` in this scope
 --> src/lib.rs:2:5
  |
2 |     let val = 42;
  |     ^^^
  = help: a local variable with a similar name exists
"#,
        temp.path().display()
    );

    // Renders with verbose: true to test printing command and cwd
    render_compiler_diagnostics(&structured_stderr, true);

    // 2. Unit failure section with no rustc error patterns (fallback diagnostic)
    let fallback_stderr = format!(
        r#"rustc failed for unit 'empty_crate@0.1.0::lib:empty_crate' (exit code 101):
  command: "rustc" "--crate-name" "empty_crate"
  cwd: {}
fatal runtime abort in rustc
"#,
        temp.path().display()
    );
    render_compiler_diagnostics(&fallback_stderr, false);
}

#[test]
fn test_m8_inputs_unit_hashing_and_bare_config() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = TempDir::new().unwrap();
    let pkg_dir = temp.path().join("bare_cfg_pkg");
    fs::create_dir_all(&pkg_dir).unwrap();

    // 1. Missing Cargo.toml bails
    assert!(hash_package_base_inputs(&pkg_dir, None, false).is_err());

    fs::write(
        pkg_dir.join("Cargo.toml"),
        br#"[package]
name = "bare_cfg_pkg"
version = "0.1.0"
edition = "2021"
"#,
    )
    .unwrap();

    // 2. Bare .cargo/config (without .toml)
    let cargo_cfg_dir = pkg_dir.join(".cargo");
    fs::create_dir_all(&cargo_cfg_dir).unwrap();
    fs::write(cargo_cfg_dir.join("config"), b"[build]\nrustflags = []\n").unwrap();

    // 3. Global CARGO_HOME/config
    let cargo_home = temp.path().join("cargo_home");
    fs::create_dir_all(&cargo_home).unwrap();
    fs::write(cargo_home.join("config"), b"[net]\noffline = true\n").unwrap();
    std::env::set_var("CARGO_HOME", &cargo_home);

    let base = hash_package_base_inputs(&pkg_dir, None, true).unwrap();
    assert!(base.iter().any(|(k, _)| k == ".cargo/config"));
    assert!(base.iter().any(|(k, _)| k == "CARGO_HOME/config"));

    // 4. append_unit_dep_info_inputs with non-rs asset
    let non_rs_asset = pkg_dir.join("asset.dat");
    fs::write(&non_rs_asset, b"asset_data_bytes").unwrap();
    let appended =
        append_unit_dep_info_inputs(base.clone(), &pkg_dir, &pkg_dir, "bare_cfg_pkg").unwrap();
    assert_eq!(appended.len(), base.len());

    // 5. hash_unit_inputs helper
    let unit_inputs = hash_unit_inputs(&pkg_dir, None, &pkg_dir, "bare_cfg_pkg", true).unwrap();
    assert!(!unit_inputs.is_empty());

    std::env::remove_var("CARGO_HOME");
}

#[test]
fn test_m8_compiler_invocation_non_semver_and_options() {
    let temp = TempDir::new().unwrap();
    let out_dir = temp.path().join("out");
    fs::create_dir_all(&out_dir).unwrap();

    let dummy_rs = temp.path().join("lib.rs");
    fs::write(&dummy_rs, b"pub fn f() {}\n").unwrap();

    let unit = PlannedCompilationUnit {
        package_id: "non_semver_pkg@1.0-custom".to_string(),
        package_name: "non_semver_pkg".to_string(),
        version: "1.0-custom".to_string(),
        target_name: "non_semver_pkg".to_string(),
        kind: "lib".to_string(),
        src_path: dummy_rs,
        manifest_path: temp.path().join("Cargo.toml"),
        cache_eligibility: "Uncached".to_string(),
        is_workspace: true,
        is_primary: false,
        env_vars: Vec::new(),
        dependencies: Vec::new(),
        edition: Some("2021".to_string()),
        features: Vec::new(),
        for_host: false,
        workspace_root: None,
    };

    let inv = RustcInvocation {
        unit: &unit,
        action_key: "short_key_123",
        out_dir: &out_dir,
        dest_binary: None,
        extern_crates: &[],
        release: false,
        features: &[],
        directives: None,
        mode: BuildMode::Build,
        extra_compiler_args: &[],
        jobserver_auth: None,
        deps_dir: None,
        target_dir: None,
    };

    let res = inv.execute().unwrap();
    assert!(res.success);
    assert!(res.primary_output.is_some());
}
