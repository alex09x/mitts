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
use std::collections::HashMap;
use std::fs;

fn make_unit_for_build_script(pkg_dir: &std::path::Path) -> PlannedCompilationUnit {
    PlannedCompilationUnit {
        package_id: "test-bs 0.1.0".to_string(),
        package_name: "test-bs".to_string(),
        version: "0.1.0".to_string(),
        target_name: "test_bs".to_string(),
        kind: "lib".to_string(),
        src_path: pkg_dir.join("src").join("lib.rs"),
        manifest_path: pkg_dir.join("Cargo.toml"),
        cache_eligibility: "eligible".to_string(),
        is_workspace: true,
        is_primary: false,
        env_vars: vec![],
        dependencies: vec![],
        edition: Some("2021".to_string()),
        features: vec!["custom_feat".to_string()],
    }
}

#[test]
fn test_m8_build_script_compile_error() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let pkg_dir = temp_dir.path().join("pkg");
    let staging_dir = temp_dir.path().join("stage");
    fs::create_dir_all(&pkg_dir).unwrap();
    fs::create_dir_all(&staging_dir).unwrap();

    fs::write(
        pkg_dir.join("Cargo.toml"),
        "[package]\nname = \"test-bs\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    // Invalid Rust syntax in build.rs
    fs::write(pkg_dir.join("build.rs"), "fn main() { syntax error !!! }\n").unwrap();

    let unit = make_unit_for_build_script(&pkg_dir);
    let res = execute_build_script(&unit, &staging_dir, false, &[], None, &[], None);
    assert!(res.is_err(), "Invalid build.rs must fail compilation");
    let err_msg = res.unwrap_err().to_string();
    assert!(err_msg.contains("Failed to compile build.rs"));
}

#[test]
fn test_m8_build_script_runtime_error() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let pkg_dir = temp_dir.path().join("pkg");
    let staging_dir = temp_dir.path().join("stage");
    fs::create_dir_all(&pkg_dir).unwrap();
    fs::create_dir_all(&staging_dir).unwrap();

    fs::write(
        pkg_dir.join("Cargo.toml"),
        "[package]\nname = \"test-bs\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    // Build script that compiles but panics at runtime
    fs::write(
        pkg_dir.join("build.rs"),
        "fn main() { panic!(\"intentional build script runtime failure\"); }\n",
    )
    .unwrap();

    let unit = make_unit_for_build_script(&pkg_dir);
    let res = execute_build_script(&unit, &staging_dir, false, &[], None, &[], None);
    assert!(res.is_err(), "Panicking build.rs must fail execution");
    let err_msg = res.unwrap_err().to_string();
    assert!(err_msg.contains("build.rs failed"));
}

#[test]
fn test_m8_build_script_success_with_links_and_extra_env() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let pkg_dir = temp_dir.path().join("pkg");
    let staging_dir = temp_dir.path().join("stage");
    fs::create_dir_all(&pkg_dir).unwrap();
    fs::create_dir_all(&staging_dir).unwrap();

    fs::write(
        pkg_dir.join("Cargo.toml"),
        "[package]\nname = \"test-bs\"\nversion = \"0.1.0\"\nlinks = \"my_sys_c\"\n",
    )
    .unwrap();

    fs::write(
        pkg_dir.join("build.rs"),
        r#"
fn main() {
    println!("cargo:rustc-cfg=has_foo");
    println!("cargo:rustc-link-lib=static=my_sys_c");
    println!("cargo:include_dir=/opt/my_sys_c/include");
}
"#,
    )
    .unwrap();

    let mut extra = HashMap::new();
    extra.insert("CUSTOM_INJECTED_VAR".to_string(), "val123".to_string());

    let unit = make_unit_for_build_script(&pkg_dir);
    let directives = execute_build_script(
        &unit,
        &staging_dir,
        false,
        &["custom_feat".to_string()],
        None,
        &[],
        Some(&extra),
    )
    .expect("execute build script");

    assert_eq!(directives.links, Some("my_sys_c".to_string()));
    assert_eq!(directives.cfgs, vec!["has_foo"]);
    assert_eq!(directives.link_libs, vec!["static=my_sys_c"]);
    assert_eq!(
        directives.metadata.get("include_dir").unwrap(),
        "/opt/my_sys_c/include"
    );
    assert!(directives.out_dir.is_some());

    let dep_vars = directives.dep_env_vars();
    assert_eq!(
        dep_vars.get("DEP_MY_SYS_C_INCLUDE_DIR").unwrap(),
        "/opt/my_sys_c/include"
    );
}
