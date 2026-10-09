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
use mitts::adapter::rust::types::{PlannedCompilationUnit, RustBuildOptions};
use mitts::adapter::rust::RustAdapter;
use mitts::cli::run::handle_run;
use mitts::cli::{BuildArgs, RunArgs};
use std::fs;
use tempfile::TempDir;

#[test]
fn test_m8_run_cli_compilation_and_runtime_failures() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    // 1. Run on broken syntax crate fails compilation
    fs::write(
        root.join("Cargo.toml"),
        r#"
[package]
name = "run_fail"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "run_fail_bin"
path = "src/main.rs"
"#,
    )
    .unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/main.rs"), "syntax error!").unwrap();

    let args_comp_fail = RunArgs {
        build_args: BuildArgs {
            manifest_path: Some(root.join("Cargo.toml")),
            ..Default::default()
        },
        trailing_args: Vec::new(),
    };
    let code_comp_fail = handle_run(&args_comp_fail);
    assert_ne!(code_comp_fail, 0);

    // 2. Run on crate that compiles but main() exits with 42
    fs::write(
        root.join("src/main.rs"),
        r#"
fn main() {
    std::process::exit(42);
}
"#,
    )
    .unwrap();
    let args_runtime_fail = RunArgs {
        build_args: BuildArgs {
            manifest_path: Some(root.join("Cargo.toml")),
            ..Default::default()
        },
        trailing_args: Vec::new(),
    };
    let code_runtime_fail = handle_run(&args_runtime_fail);
    assert_eq!(code_runtime_fail, 42);
}

#[test]
fn test_m8_build_script_runtime_failure_error_formatting() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    // Build script that exits with code 5 and prints error
    let build_src = root.join("build.rs");
    fs::write(
        &build_src,
        r#"
fn main() {
    eprintln!("custom build script fatal error");
    std::process::exit(5);
}
"#,
    )
    .unwrap();
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"bs_fail\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();

    let unit = PlannedCompilationUnit {
        package_id: "bs_fail".to_string(),
        package_name: "bs_fail".to_string(),
        version: "0.1.0".to_string(),
        target_name: "bs_fail".to_string(),
        kind: "custom-build".to_string(),
        src_path: build_src,
        manifest_path: root.join("Cargo.toml"),
        cache_eligibility: "test".to_string(),
        is_workspace: true,
        is_primary: false,
        env_vars: Vec::new(),
        dependencies: Vec::new(),
        edition: Some("2021".to_string()),
        features: Vec::new(),
    };

    let target_dir = root.join("target");
    fs::create_dir_all(&target_dir).unwrap();

    let err = execute_build_script(&unit, &target_dir, false, &[], None, &[], None);
    assert!(err.is_err());
    let err_str = err.unwrap_err().to_string();
    assert!(
        err_str.contains("build.rs failed for 'bs_fail'")
            && err_str.contains("custom build script fatal error")
    );
}

#[test]
fn test_m8_planner_dash_underscore_dependency_resolution() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    fs::write(
        root.join("Cargo.toml"),
        r#"
[package]
name = "main_pkg"
version = "0.1.0"
edition = "2021"

[dependencies]
sub-lib = { path = "sub-lib" }

[[bin]]
name = "main_bin"
path = "src/main.rs"
"#,
    )
    .unwrap();

    let sub_dir = root.join("sub-lib");
    fs::create_dir_all(&sub_dir).unwrap();
    fs::write(
        sub_dir.join("Cargo.toml"),
        r#"
[package]
name = "sub-lib"
version = "0.2.0"
edition = "2021"

[lib]
path = "src/lib.rs"
"#,
    )
    .unwrap();
    fs::create_dir_all(sub_dir.join("src")).unwrap();
    fs::write(sub_dir.join("src/lib.rs"), "pub fn val() -> i32 { 1 }").unwrap();

    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();

    let opt = RustBuildOptions {
        manifest_path: Some(root.join("Cargo.toml")),
        ..Default::default()
    };
    let exp = RustAdapter::explain_plan(&opt).unwrap();
    assert_eq!(exp.units.len(), 2);

    let bin_unit = exp.units.iter().find(|u| u.kind == "bin").unwrap();
    assert!(!bin_unit.dependencies.is_empty());
}
