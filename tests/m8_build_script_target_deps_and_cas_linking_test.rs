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
use std::collections::HashMap;
use std::fs;
use tempfile::TempDir;

#[test]
fn test_m8_build_script_directives_parse_custom_metadata() {
    let stdout = r#"
cargo:rustc-cfg=feature_a
cargo:rustc-link-lib=sqlite3
cargo:rustc-link-search=/usr/local/lib
cargo:rustc-flags=-L /opt/custom
cargo:rustc-env=MY_CUSTOM_VAR=123
cargo:rerun-if-changed=build.rs
cargo:custom_meta=some_value
cargo:another_key=hello_world
"#;

    let directives = BuildScriptDirectives::parse_stdout(stdout);
    assert!(directives.cfgs.contains(&"feature_a".to_string()));
    assert!(directives.link_libs.contains(&"sqlite3".to_string()));
    assert!(directives
        .link_searches
        .contains(&"/usr/local/lib".to_string()));
    assert_eq!(
        directives.envs.get("MY_CUSTOM_VAR"),
        Some(&"123".to_string())
    );
    assert_eq!(
        directives.metadata.get("custom_meta"),
        Some(&"some_value".to_string())
    );
    assert_eq!(
        directives.metadata.get("another_key"),
        Some(&"hello_world".to_string())
    );
}

#[test]
fn test_m8_execute_build_script_target_build_dependencies_linking() {
    let temp = TempDir::new().unwrap();
    let pkg_dir = temp.path().join("my_crate");
    let src_dir = pkg_dir.join("src");
    let staging_dir = temp.path().join("staging");
    let cas_artifacts = temp.path().join("cas_artifacts");

    fs::create_dir_all(&src_dir).unwrap();
    fs::create_dir_all(&staging_dir).unwrap();
    fs::create_dir_all(&cas_artifacts).unwrap();

    // Manifest with target-specific build-dependencies
    let manifest_path = pkg_dir.join("Cargo.toml");
    fs::write(
        &manifest_path,
        r#"
[package]
name = "my_crate"
version = "0.1.0"
edition = "2021"

[build-dependencies]
build_helper = "0.1.0"

[target.'cfg(unix)'.build-dependencies]
unix_helper = "0.2.0"
"#,
    )
    .unwrap();

    // Dummy build.rs that outputs directives
    let build_rs = pkg_dir.join("build.rs");
    fs::write(
        &build_rs,
        r#"
fn main() {
    println!("cargo:rustc-cfg=has_built");
    println!("cargo:custom_flag=true");
}
"#,
    )
    .unwrap();

    // Dummy rlib in cas_artifacts for build_helper and unix_helper
    fs::write(
        cas_artifacts.join("libbuild_helper-12345678.rlib"),
        b"mock rlib",
    )
    .unwrap();
    fs::write(
        cas_artifacts.join("libunix_helper-87654321.rlib"),
        b"mock rlib",
    )
    .unwrap();

    let unit = PlannedCompilationUnit {
        package_id: "my_crate@0.1.0".to_string(),
        package_name: "my_crate".to_string(),
        version: "0.1.0".to_string(),
        target_name: "my_crate".to_string(),
        kind: "lib".to_string(),
        src_path: src_dir.join("lib.rs"),
        manifest_path,
        cache_eligibility: "eligible".to_string(),
        is_workspace: true,
        is_primary: false,
        env_vars: Vec::new(),
        dependencies: Vec::new(),
        edition: Some("2021".to_string()),
        features: Vec::new(),
    };

    let extra_env = HashMap::new();
    let directives = execute_build_script(
        &unit,
        &staging_dir,
        false,
        &[],
        Some(&cas_artifacts),
        &[],
        Some(&extra_env),
    )
    .unwrap();

    assert!(directives.cfgs.contains(&"has_built".to_string()));
    assert_eq!(
        directives.metadata.get("custom_flag"),
        Some(&"true".to_string())
    );
}

#[test]
fn test_m8_execute_build_script_compilation_failure() {
    let temp = TempDir::new().unwrap();
    let pkg_dir = temp.path().join("broken_crate");
    let staging_dir = temp.path().join("staging");
    fs::create_dir_all(&pkg_dir).unwrap();
    fs::create_dir_all(&staging_dir).unwrap();

    let manifest_path = pkg_dir.join("Cargo.toml");
    fs::write(
        &manifest_path,
        "[package]\nname = \"broken_crate\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();

    // Syntax error in build.rs
    fs::write(pkg_dir.join("build.rs"), "syntax error here !!!").unwrap();

    let unit = PlannedCompilationUnit {
        package_id: "broken_crate@0.1.0".to_string(),
        package_name: "broken_crate".to_string(),
        version: "0.1.0".to_string(),
        target_name: "broken_crate".to_string(),
        kind: "lib".to_string(),
        src_path: pkg_dir.join("src").join("lib.rs"),
        manifest_path,
        cache_eligibility: "eligible".to_string(),
        is_workspace: true,
        is_primary: false,
        env_vars: Vec::new(),
        dependencies: Vec::new(),
        edition: Some("2021".to_string()),
        features: Vec::new(),
    };

    let res = execute_build_script(&unit, &staging_dir, false, &[], None, &[], None);
    assert!(res.is_err());
    assert!(res
        .unwrap_err()
        .to_string()
        .contains("Failed to compile build.rs for 'broken_crate'"));
}
