/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::rust::manifest::CargoToml;
use mitts::cli::run::handle_run;
use mitts::cli::{BuildArgs, RunArgs};
use std::fs;
use tempfile::TempDir;

#[test]
fn test_m8_run_no_binary_target_and_build_failure() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    // 1. Library-only package has no binary target -> handle_run returns 1
    let lib_manifest = root.join("lib_pkg/Cargo.toml");
    fs::create_dir_all(root.join("lib_pkg/src")).unwrap();
    fs::write(
        &lib_manifest,
        "[package]\nname = \"lib_only\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(
        root.join("lib_pkg/src/lib.rs"),
        "pub fn val() -> i32 { 1 }\n",
    )
    .unwrap();

    let args_lib = RunArgs {
        build_args: BuildArgs {
            manifest_path: Some(lib_manifest),
            ..Default::default()
        },
        ..Default::default()
    };
    let code_lib = handle_run(&args_lib);
    assert_eq!(code_lib, 1);

    // 2. Compilation failure in binary crate -> handle_run returns compilation exit code
    let fail_manifest = root.join("fail_pkg/Cargo.toml");
    fs::create_dir_all(root.join("fail_pkg/src")).unwrap();
    fs::write(
        &fail_manifest,
        "[package]\nname = \"fail_bin\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(
        root.join("fail_pkg/src/main.rs"),
        "fn main() { invalid syntax here !!! }\n",
    )
    .unwrap();

    let args_fail = RunArgs {
        build_args: BuildArgs {
            manifest_path: Some(fail_manifest),
            ..Default::default()
        },
        ..Default::default()
    };
    let code_fail = handle_run(&args_fail);
    assert_ne!(code_fail, 0);
}

#[test]
fn test_m8_manifest_target_build_dependencies_discovery() {
    let toml_str = r#"
[package]
name = "target_dep_pkg"
version = "0.1.0"
edition = "2021"

[target.'cfg(target_os = "linux")'.dependencies]
linux_dep = { package = "real_linux_dep", version = "1.0" }

[target.'cfg(target_os = "linux")'.build-dependencies]
linux_build_tool = { package = "real_build_tool", version = "0.2" }
"#;
    let toml: CargoToml = toml::from_str(toml_str).unwrap();
    let aliases = toml.dependency_aliases();
    assert_eq!(
        aliases.alias_to_real.get("linux_dep"),
        Some(&"real_linux_dep".to_string())
    );
    assert_eq!(
        aliases.alias_to_real.get("linux_build_tool"),
        Some(&"real_build_tool".to_string())
    );
}
