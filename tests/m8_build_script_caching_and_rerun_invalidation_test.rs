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
use mitts::adapter::rust::types::PlannedCompilationUnit;
use std::fs;
use tempfile::TempDir;

#[test]
fn test_build_script_rerun_if_changed_invalidation() {
    let temp = TempDir::new().unwrap();
    let pkg_dir = temp.path().join("my_pkg");
    let staging_1 = temp.path().join("staging_1");
    let cas_dir = temp.path().join("cas");
    let artifacts_dir = cas_dir.join("artifacts");
    fs::create_dir_all(&pkg_dir).unwrap();
    fs::create_dir_all(&staging_1).unwrap();
    fs::create_dir_all(&artifacts_dir).unwrap();

    let manifest = pkg_dir.join("Cargo.toml");
    fs::write(
        &manifest,
        "[package]\nname = \"my_pkg\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();

    let data_file = pkg_dir.join("data.txt");
    fs::write(&data_file, "initial data v1").unwrap();

    let build_rs = pkg_dir.join("build.rs");
    fs::write(
        &build_rs,
        r#"
use std::fs;
use std::env;
use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=data.txt");
    println!("cargo:rustc-cfg=data_loaded");
    let out_dir = env::var("OUT_DIR").unwrap();
    let dest = Path::new(&out_dir).join("out.txt");
    fs::write(dest, "generated output content").unwrap();
}
"#,
    )
    .unwrap();

    let unit = PlannedCompilationUnit {
        package_id: "my_pkg@0.1.0".to_string(),
        package_name: "my_pkg".to_string(),
        version: "0.1.0".to_string(),
        target_name: "my_pkg".to_string(),
        kind: "lib".to_string(),
        src_path: pkg_dir.join("src/lib.rs"),
        manifest_path: manifest.clone(),
        dependencies: Vec::new(),
        edition: Some("2021".to_string()),
        is_workspace: true,
        cache_eligibility: "cacheable".to_string(),
        env_vars: Vec::new(),
        features: Vec::new(),
    };

    // Run 1: Cold execution
    let d1 = execute_build_script(
        &unit,
        &staging_1,
        false,
        &[],
        Some(&artifacts_dir),
        &[],
        None,
    )
    .expect("Run 1 failed");

    assert!(d1.cfgs.contains(&"data_loaded".to_string()));
    assert_eq!(d1.rerun_if_changed, vec!["data.txt".to_string()]);
    assert!(staging_1.join("out/out.txt").is_file());

    // Run 2: Cache hit in staging_2 (data.txt untouched)
    let staging_2 = temp.path().join("staging_2");
    fs::create_dir_all(&staging_2).unwrap();

    let d2 = execute_build_script(
        &unit,
        &staging_2,
        false,
        &[],
        Some(&artifacts_dir),
        &[],
        None,
    )
    .expect("Run 2 failed");

    assert!(d2.cfgs.contains(&"data_loaded".to_string()));
    assert!(staging_2.join("out/out.txt").is_file());
    assert_eq!(
        fs::read_to_string(staging_2.join("out/out.txt")).unwrap(),
        "generated output content"
    );

    // Run 3: Modify data.txt -> Must trigger rerun and invalidation
    std::thread::sleep(std::time::Duration::from_millis(15));
    fs::write(&data_file, "modified data v2").unwrap();

    let staging_3 = temp.path().join("staging_3");
    fs::create_dir_all(&staging_3).unwrap();

    let d3 = execute_build_script(
        &unit,
        &staging_3,
        false,
        &[],
        Some(&artifacts_dir),
        &[],
        None,
    )
    .expect("Run 3 failed");

    assert!(d3.cfgs.contains(&"data_loaded".to_string()));
    assert!(staging_3.join("out/out.txt").is_file());
}

#[test]
fn test_build_script_rerun_if_env_changed_invalidation() {
    let temp = TempDir::new().unwrap();
    let pkg_dir = temp.path().join("env_pkg");
    let staging_1 = temp.path().join("staging_1");
    let cas_dir = temp.path().join("cas");
    let artifacts_dir = cas_dir.join("artifacts");
    fs::create_dir_all(&pkg_dir).unwrap();
    fs::create_dir_all(&staging_1).unwrap();
    fs::create_dir_all(&artifacts_dir).unwrap();

    let manifest = pkg_dir.join("Cargo.toml");
    fs::write(
        &manifest,
        "[package]\nname = \"env_pkg\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();

    let build_rs = pkg_dir.join("build.rs");
    fs::write(
        &build_rs,
        r#"
use std::env;

fn main() {
    println!("cargo:rerun-if-env-changed=MITTS_TEST_FEATURE");
    if env::var("MITTS_TEST_FEATURE").unwrap_or_default() == "enabled" {
        println!("cargo:rustc-cfg=feature_enabled");
    }
}
"#,
    )
    .unwrap();

    let unit = PlannedCompilationUnit {
        package_id: "env_pkg@0.1.0".to_string(),
        package_name: "env_pkg".to_string(),
        version: "0.1.0".to_string(),
        target_name: "env_pkg".to_string(),
        kind: "lib".to_string(),
        src_path: pkg_dir.join("src/lib.rs"),
        manifest_path: manifest.clone(),
        dependencies: Vec::new(),
        edition: Some("2021".to_string()),
        is_workspace: true,
        cache_eligibility: "cacheable".to_string(),
        env_vars: Vec::new(),
        features: Vec::new(),
    };

    // Run 1: with MITTS_TEST_FEATURE=enabled
    std::env::set_var("MITTS_TEST_FEATURE", "enabled");
    let d1 = execute_build_script(
        &unit,
        &staging_1,
        false,
        &[],
        Some(&artifacts_dir),
        &[],
        None,
    )
    .expect("Run 1 failed");
    assert!(d1.cfgs.contains(&"feature_enabled".to_string()));

    // Run 2: with MITTS_TEST_FEATURE=enabled -> Cache hit
    let staging_2 = temp.path().join("staging_2");
    fs::create_dir_all(&staging_2).unwrap();
    let d2 = execute_build_script(
        &unit,
        &staging_2,
        false,
        &[],
        Some(&artifacts_dir),
        &[],
        None,
    )
    .expect("Run 2 failed");
    assert!(d2.cfgs.contains(&"feature_enabled".to_string()));

    // Run 3: change env var to "disabled" -> Invalidation and rerun
    std::env::set_var("MITTS_TEST_FEATURE", "disabled");
    let staging_3 = temp.path().join("staging_3");
    fs::create_dir_all(&staging_3).unwrap();
    let d3 = execute_build_script(
        &unit,
        &staging_3,
        false,
        &[],
        Some(&artifacts_dir),
        &[],
        None,
    )
    .expect("Run 3 failed");
    assert!(!d3.cfgs.contains(&"feature_enabled".to_string()));

    std::env::remove_var("MITTS_TEST_FEATURE");
}

#[test]
fn test_build_script_workspace_default_invalidation() {
    let temp = TempDir::new().unwrap();
    let pkg_dir = temp.path().join("ws_pkg");
    let staging_1 = temp.path().join("staging_1");
    let cas_dir = temp.path().join("cas");
    let artifacts_dir = cas_dir.join("artifacts");
    fs::create_dir_all(pkg_dir.join("src")).unwrap();
    fs::create_dir_all(&staging_1).unwrap();
    fs::create_dir_all(&artifacts_dir).unwrap();

    let manifest = pkg_dir.join("Cargo.toml");
    fs::write(
        &manifest,
        "[package]\nname = \"ws_pkg\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();

    let lib_rs = pkg_dir.join("src/lib.rs");
    fs::write(&lib_rs, "pub fn initial() {}").unwrap();

    // build.rs without any rerun-if directives
    let build_rs = pkg_dir.join("build.rs");
    fs::write(
        &build_rs,
        r#"
fn main() {
    println!("cargo:rustc-cfg=workspace_ready");
}
"#,
    )
    .unwrap();

    let unit = PlannedCompilationUnit {
        package_id: "ws_pkg@0.1.0".to_string(),
        package_name: "ws_pkg".to_string(),
        version: "0.1.0".to_string(),
        target_name: "ws_pkg".to_string(),
        kind: "lib".to_string(),
        src_path: lib_rs.clone(),
        manifest_path: manifest.clone(),
        dependencies: Vec::new(),
        edition: Some("2021".to_string()),
        is_workspace: true,
        cache_eligibility: "cacheable".to_string(),
        env_vars: Vec::new(),
        features: Vec::new(),
    };

    // Run 1: First build
    let d1 = execute_build_script(
        &unit,
        &staging_1,
        false,
        &[],
        Some(&artifacts_dir),
        &[],
        None,
    )
    .unwrap();
    assert!(d1.cfgs.contains(&"workspace_ready".to_string()));

    // Run 2: Unmodified package -> Cache hit
    let staging_2 = temp.path().join("staging_2");
    fs::create_dir_all(&staging_2).unwrap();
    let d2 = execute_build_script(
        &unit,
        &staging_2,
        false,
        &[],
        Some(&artifacts_dir),
        &[],
        None,
    )
    .unwrap();
    assert!(d2.cfgs.contains(&"workspace_ready".to_string()));

    // Modify a package source file in workspace crate
    std::thread::sleep(std::time::Duration::from_millis(15));
    fs::write(&lib_rs, "pub fn modified_code() {}").unwrap();

    let staging_3 = temp.path().join("staging_3");
    fs::create_dir_all(&staging_3).unwrap();
    let d3 = execute_build_script(
        &unit,
        &staging_3,
        false,
        &[],
        Some(&artifacts_dir),
        &[],
        None,
    )
    .unwrap();
    assert!(d3.cfgs.contains(&"workspace_ready".to_string()));
}

#[test]
fn test_build_script_restore_out_dir_in_clean_checkout() {
    let temp = TempDir::new().unwrap();
    let pkg_dir = temp.path().join("gen_pkg");
    let staging_1 = temp.path().join("staging_1");
    let cas_dir = temp.path().join("cas");
    let artifacts_dir = cas_dir.join("artifacts");
    fs::create_dir_all(&pkg_dir).unwrap();
    fs::create_dir_all(&staging_1).unwrap();
    fs::create_dir_all(&artifacts_dir).unwrap();

    let manifest = pkg_dir.join("Cargo.toml");
    fs::write(
        &manifest,
        "[package]\nname = \"gen_pkg\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();

    let build_rs = pkg_dir.join("build.rs");
    fs::write(
        &build_rs,
        r#"
use std::fs;
use std::env;
use std::path::Path;

fn main() {
    let out_dir = env::var("OUT_DIR").unwrap();
    let gen_file = Path::new(&out_dir).join("generated.rs");
    fs::write(gen_file, "pub const MAGIC_NUMBER: u32 = 42;").unwrap();
}
"#,
    )
    .unwrap();

    let unit = PlannedCompilationUnit {
        package_id: "gen_pkg@0.1.0".to_string(),
        package_name: "gen_pkg".to_string(),
        version: "0.1.0".to_string(),
        target_name: "gen_pkg".to_string(),
        kind: "lib".to_string(),
        src_path: pkg_dir.join("src/lib.rs"),
        manifest_path: manifest.clone(),
        dependencies: Vec::new(),
        edition: Some("2021".to_string()),
        is_workspace: true,
        cache_eligibility: "cacheable".to_string(),
        env_vars: Vec::new(),
        features: Vec::new(),
    };

    // First build populates CAS and generates OUT_DIR/generated.rs
    let _d1 = execute_build_script(
        &unit,
        &staging_1,
        false,
        &[],
        Some(&artifacts_dir),
        &[],
        None,
    )
    .expect("First run failed");
    assert!(staging_1.join("out/generated.rs").is_file());

    // Completely new clean checkout directory
    let clean_checkout_staging = temp.path().join("clean_checkout_staging");
    fs::create_dir_all(&clean_checkout_staging).unwrap();
    assert!(!clean_checkout_staging.join("out").exists());

    // Second run restores OUT_DIR files from CAS
    let _d2 = execute_build_script(
        &unit,
        &clean_checkout_staging,
        false,
        &[],
        Some(&artifacts_dir),
        &[],
        None,
    )
    .expect("Second run restore failed");

    let restored_file = clean_checkout_staging.join("out/generated.rs");
    assert!(
        restored_file.is_file(),
        "OUT_DIR/generated.rs was not restored into clean checkout!"
    );
    let content = fs::read_to_string(&restored_file).unwrap();
    assert_eq!(content, "pub const MAGIC_NUMBER: u32 = 42;");
}
