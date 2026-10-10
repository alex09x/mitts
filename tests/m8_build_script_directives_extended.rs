/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::rust::build_script::{execute_build_script, BuildScriptDirectives};
use mitts::adapter::rust::types::PlannedCompilationUnit;
use std::fs;
use tempfile::TempDir;

#[test]
fn test_m8_build_script_directives_parsing_and_dep_env() {
    let stdout = r#"
cargo:rustc-cfg=has_feature_z
cargo::rustc-link-lib=sqlite3
cargo:rustc-link-search=native=/opt/lib
cargo::rustc-env=GENERATED_VAR=hello_mitts
cargo:rerun-if-changed=build.rs
cargo:warning=this is a build warning
cargo:rustc-check-cfg=cfg(foo)
cargo:metadata=lib_key=lib_val
cargo:custom_meta=custom_val
"#;

    let mut dir = BuildScriptDirectives::parse_stdout(stdout);
    assert!(!dir.is_empty());
    assert_eq!(dir.cfgs, vec!["has_feature_z"]);
    assert_eq!(dir.link_libs, vec!["sqlite3"]);
    assert_eq!(dir.link_searches, vec!["/opt/lib"]);
    assert_eq!(dir.envs.get("GENERATED_VAR").unwrap(), "hello_mitts");
    assert_eq!(dir.metadata.get("lib_key").unwrap(), "lib_val");
    assert_eq!(dir.metadata.get("custom_meta").unwrap(), "custom_val");

    // Test dep_env_vars when links is None vs Some
    assert!(dir.dep_env_vars().is_empty());

    dir.links = Some("native-lib".to_string());
    let env_vars = dir.dep_env_vars();
    assert_eq!(env_vars.get("DEP_NATIVE_LIB_LIB_KEY").unwrap(), "lib_val");
    assert_eq!(
        env_vars.get("DEP_NATIVE_LIB_CUSTOM_META").unwrap(),
        "custom_val"
    );
}

#[test]
fn test_m8_build_script_custom_path_and_disabled() {
    let tmp = TempDir::new().unwrap();
    let pkg_dir = tmp.path();

    // 1. Build script disabled via build = false
    let toml_disabled = pkg_dir.join("Cargo.toml");
    fs::write(
        &toml_disabled,
        "[package]\nname = \"disabled-build\"\nversion = \"0.1.0\"\nbuild = false\n",
    )
    .unwrap();

    let unit = PlannedCompilationUnit {
        package_id: "disabled-build 0.1.0".to_string(),
        package_name: "disabled-build".to_string(),
        version: "0.1.0".to_string(),
        target_name: "disabled_build".to_string(),
        kind: "lib".to_string(),
        src_path: pkg_dir.join("src").join("lib.rs"),
        manifest_path: toml_disabled,
        dependencies: Vec::new(),
        features: Vec::new(),
        env_vars: Vec::new(),
        cache_eligibility: "eligible".to_string(),
        is_workspace: true,
        is_primary: false,
        edition: Some("2021".to_string()),
        for_host: false,
        workspace_root: None,
    };

    let staging_dir = tmp.path().join("staging");
    fs::create_dir_all(&staging_dir).unwrap();

    let res = execute_build_script(&unit, &staging_dir, false, &[], None, &[], None).unwrap();
    assert!(res.is_empty());

    // 2. Custom build path: build = "custom_build.rs"
    let toml_custom = pkg_dir.join("Cargo.toml");
    fs::write(
        &toml_custom,
        "[package]\nname = \"custom-build\"\nversion = \"0.1.0\"\nbuild = \"custom_build.rs\"\n",
    )
    .unwrap();
    let custom_rs = pkg_dir.join("custom_build.rs");
    fs::write(
        &custom_rs,
        "fn main() { println!(\"cargo:rustc-cfg=custom_active\"); }",
    )
    .unwrap();

    let mut unit_custom = unit.clone();
    unit_custom.package_name = "custom-build".to_string();
    unit_custom.manifest_path = toml_custom;

    let res2 =
        execute_build_script(&unit_custom, &staging_dir, false, &[], None, &[], None).unwrap();
    assert_eq!(res2.cfgs, vec!["custom_active"]);
    assert!(res2.out_dir.is_some());
}

#[test]
fn test_m8_build_script_nested_build_dir_fallback() {
    let tmp = TempDir::new().unwrap();
    let pkg_dir = tmp.path();

    let toml_path = pkg_dir.join("Cargo.toml");
    fs::write(
        &toml_path,
        "[package]\nname = \"nested-pkg\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();

    // build/build.rs nested location
    let nested_dir = pkg_dir.join("build");
    fs::create_dir_all(&nested_dir).unwrap();
    fs::write(
        nested_dir.join("build.rs"),
        "fn main() { println!(\"cargo:rustc-cfg=nested_active\"); }",
    )
    .unwrap();

    let unit = PlannedCompilationUnit {
        package_id: "nested-pkg 0.1.0".to_string(),
        package_name: "nested-pkg".to_string(),
        version: "0.1.0".to_string(),
        target_name: "nested_pkg".to_string(),
        kind: "lib".to_string(),
        src_path: pkg_dir.join("src").join("lib.rs"),
        manifest_path: toml_path,
        dependencies: Vec::new(),
        features: Vec::new(),
        env_vars: Vec::new(),
        cache_eligibility: "eligible".to_string(),
        is_workspace: true,
        is_primary: false,
        edition: Some("2021".to_string()),
        for_host: false,
        workspace_root: None,
    };

    let staging_dir = tmp.path().join("staging");
    fs::create_dir_all(&staging_dir).unwrap();

    let res = execute_build_script(&unit, &staging_dir, false, &[], None, &[], None).unwrap();
    assert_eq!(res.cfgs, vec!["nested_active"]);
}
