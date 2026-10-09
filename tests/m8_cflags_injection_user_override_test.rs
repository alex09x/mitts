/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::rust::build_script::execute_build_script;
use mitts::adapter::rust::build_script_env::configure_build_script_run_cmd;
use mitts::adapter::rust::manifest::CargoToml;
use mitts::adapter::rust::types::PlannedCompilationUnit;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

static SERIAL_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn make_unit(pkg_dir: &Path) -> PlannedCompilationUnit {
    PlannedCompilationUnit {
        package_id: "c-user-flags 0.1.0".to_string(),
        package_name: "c-user-flags".to_string(),
        version: "0.1.0".to_string(),
        target_name: "c_user_flags".to_string(),
        kind: "lib".to_string(),
        src_path: pkg_dir.join("src/lib.rs"),
        manifest_path: pkg_dir.join("Cargo.toml"),
        cache_eligibility: "eligible".to_string(),
        is_workspace: true,
        is_primary: false,
        env_vars: vec![],
        dependencies: vec![],
        edition: Some("2021".to_string()),
        features: vec![],
    }
}

#[test]
fn test_m8_cflags_user_override_preserved_without_shadowing() {
    let _lock = SERIAL_TEST_LOCK.lock().unwrap();

    let temp = TempDir::new().unwrap();
    let pkg_dir = temp.path().join("pkg");
    let out_dir = temp.path().join("out");
    fs::create_dir_all(&pkg_dir).unwrap();
    fs::create_dir_all(&out_dir).unwrap();

    let unit = make_unit(&pkg_dir);
    let pkg_toml = CargoToml::default();

    let target = "x86_64-unknown-linux-gnu";
    let target_var = format!("CFLAGS_{}", target.replace('-', "_"));

    // Scenario A: User specifies only CFLAGS="-DMITTS_MARK=1"
    let mut extra_env = HashMap::new();
    extra_env.insert("CFLAGS".to_string(), "-DMITTS_MARK=1".to_string());

    let cmd = configure_build_script_run_cmd(
        &PathBuf::from("/bin/true"),
        &pkg_dir,
        &unit,
        &pkg_toml,
        &out_dir,
        target,
        false,
        &[],
        Some(&extra_env),
    );

    let env_map: HashMap<String, Option<String>> = cmd
        .get_envs()
        .map(|(k, v)| {
            (
                k.to_string_lossy().to_string(),
                v.map(|s| s.to_string_lossy().to_string()),
            )
        })
        .collect();

    // 1. CFLAGS must contain user flag and prefix map
    let cflags = env_map.get("CFLAGS").and_then(|v| v.as_ref()).unwrap();
    assert!(
        cflags.contains("-DMITTS_MARK=1"),
        "User CFLAGS must be preserved, got: {}",
        cflags
    );
    assert!(
        cflags.contains("-ffile-prefix-map="),
        "prefix-map must be appended, got: {}",
        cflags
    );

    // 2. TARGET_CFLAGS and CFLAGS_<target> must NOT be created, preventing shadowing in cc-rs
    assert!(
        !env_map.contains_key("TARGET_CFLAGS"),
        "TARGET_CFLAGS must not be set when user did not set it"
    );
    assert!(
        !env_map.contains_key(&target_var),
        "CFLAGS_<target> must not be set when user did not set it"
    );

    // Scenario B: MSVC target must NOT inject -ffile-prefix-map
    let msvc_target = "x86_64-pc-windows-msvc";
    let cmd_msvc = configure_build_script_run_cmd(
        &PathBuf::from("/bin/true"),
        &pkg_dir,
        &unit,
        &pkg_toml,
        &out_dir,
        msvc_target,
        false,
        &[],
        Some(&extra_env),
    );
    let msvc_env: HashMap<String, Option<String>> = cmd_msvc
        .get_envs()
        .map(|(k, v)| {
            (
                k.to_string_lossy().to_string(),
                v.map(|s| s.to_string_lossy().to_string()),
            )
        })
        .collect();
    let msvc_cflags = msvc_env.get("CFLAGS").and_then(|v| v.as_ref()).unwrap();
    assert_eq!(
        msvc_cflags, "-DMITTS_MARK=1",
        "MSVC must not receive -ffile-prefix-map"
    );

    // Scenario C: Path with spaces must NOT inject -ffile-prefix-map
    let space_out = temp.path().join("out with spaces");
    fs::create_dir_all(&space_out).unwrap();
    let cmd_space = configure_build_script_run_cmd(
        &PathBuf::from("/bin/true"),
        &pkg_dir,
        &unit,
        &pkg_toml,
        &space_out,
        target,
        false,
        &[],
        Some(&extra_env),
    );
    let space_env: HashMap<String, Option<String>> = cmd_space
        .get_envs()
        .map(|(k, v)| {
            (
                k.to_string_lossy().to_string(),
                v.map(|s| s.to_string_lossy().to_string()),
            )
        })
        .collect();
    let space_cflags = space_env.get("CFLAGS").and_then(|v| v.as_ref()).unwrap();
    assert_eq!(
        space_cflags, "-DMITTS_MARK=1",
        "Paths with spaces must not receive split -ffile-prefix-map"
    );
}

#[test]
fn test_m8_build_rs_cc_compiles_with_user_cflags() {
    let _lock = SERIAL_TEST_LOCK.lock().unwrap();

    let temp = TempDir::new().unwrap();
    let pkg_dir = temp.path().join("pkg");
    let staging_dir = temp.path().join("stage");
    fs::create_dir_all(pkg_dir.join("src")).unwrap();
    fs::create_dir_all(&staging_dir).unwrap();

    fs::write(
        pkg_dir.join("Cargo.toml"),
        r#"[package]
name = "c-user-flags"
version = "0.1.0"
edition = "2021"
"#,
    )
    .unwrap();

    // test.c requires MITTS_MARK=1 to compile successfully
    fs::write(
        pkg_dir.join("test.c"),
        r#"#ifndef MITTS_MARK
#error "MITTS_MARK is not defined in compiler flags"
#endif

int mitts_mark_val(void) {
    return MITTS_MARK;
}
"#,
    )
    .unwrap();

    // build.rs compiles test.c using the C compiler and flags from CFLAGS
    fs::write(
        pkg_dir.join("build.rs"),
        r#"use std::env;
use std::process::Command;

fn main() {
    let cflags = env::var("CFLAGS").expect("CFLAGS must be passed to build.rs");
    let out_dir = env::var("OUT_DIR").expect("OUT_DIR must be set");

    // Invoke cc with flags parsed from CFLAGS
    let mut cmd = Command::new("cc");
    for flag in cflags.split_whitespace() {
        cmd.arg(flag);
    }
    cmd.arg("-c").arg("test.c").arg("-o").arg(format!("{}/test.o", out_dir));

    let status = cmd.status().expect("failed to execute cc compiler");
    assert!(status.success(), "cc compilation failed with CFLAGS: {}", cflags);

    println!("cargo:rerun-if-changed=test.c");
    println!("cargo:rerun-if-changed=build.rs");
}
"#,
    )
    .unwrap();

    fs::write(
        pkg_dir.join("src/lib.rs"),
        "pub fn placeholder() -> i32 { 42 }\n",
    )
    .unwrap();

    let unit = make_unit(&pkg_dir);

    // Set CFLAGS with the user macro
    let mut extra_env = HashMap::new();
    extra_env.insert("CFLAGS".to_string(), "-DMITTS_MARK=1".to_string());

    let res = execute_build_script(&unit, &staging_dir, false, &[], None, &[], Some(&extra_env));

    assert!(
        res.is_ok(),
        "build.rs must succeed when user CFLAGS=-DMITTS_MARK=1 is preserved: {:?}",
        res.err()
    );

    let directives = res.unwrap();
    assert!(
        directives
            .out_dir
            .as_ref()
            .expect("out_dir present")
            .join("test.o")
            .exists(),
        "Compiled test.o must exist in OUT_DIR"
    );
}
