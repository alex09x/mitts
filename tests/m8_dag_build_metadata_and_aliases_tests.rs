/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::rust::types::RustBuildOptions;
use mitts::adapter::RustAdapter;
use std::fs;
use tempfile::TempDir;

#[test]
fn test_m8_dag_build_metadata_propagation_and_aliases() {
    let temp_dir = TempDir::new().expect("tempdir");
    let ws_dir = temp_dir.path().join("ws");
    let sys_a_dir = ws_dir.join("sys_a");
    let dep_b_dir = ws_dir.join("dep_b");
    fs::create_dir_all(&sys_a_dir).unwrap();
    fs::create_dir_all(&dep_b_dir).unwrap();

    // 1. Workspace root manifest
    fs::write(
        ws_dir.join("Cargo.toml"),
        "[workspace]\nmembers = [\"sys_a\", \"dep_b\"]\n",
    )
    .unwrap();

    // 2. sys_a crate with links = "sys-a" and build.rs emitting metadata
    fs::write(
        sys_a_dir.join("Cargo.toml"),
        r#"[package]
name = "sys-a"
version = "0.1.0"
links = "sys-a"
"#,
    )
    .unwrap();
    fs::write(
        sys_a_dir.join("build.rs"),
        r#"fn main() {
    println!("cargo:include_dir=/usr/include/sys_a");
    println!("cargo:rustc-cfg=sys_a_ready");
}
"#,
    )
    .unwrap();
    let sys_a_src = sys_a_dir.join("src");
    fs::create_dir_all(&sys_a_src).unwrap();
    fs::write(sys_a_src.join("lib.rs"), "pub fn sys_a() {}\n").unwrap();

    // 3. dep_b crate with renamed dependency alias
    fs::write(
        dep_b_dir.join("Cargo.toml"),
        r#"[package]
name = "dep-b"
version = "0.1.0"

[dependencies]
aliased_sys = { package = "sys-a", path = "../sys_a" }
"#,
    )
    .unwrap();
    let dep_b_src = dep_b_dir.join("src");
    fs::create_dir_all(&dep_b_src).unwrap();
    fs::write(
        dep_b_src.join("lib.rs"),
        r#"extern crate aliased_sys;
pub fn dep_b() {
    aliased_sys::sys_a();
}
"#,
    )
    .unwrap();

    // 4. Build workspace with native DAG runner
    let opt = RustBuildOptions {
        manifest_path: Some(ws_dir.join("Cargo.toml")),
        target_dir: Some(temp_dir.path().join("target")),
        cache_dir: Some(temp_dir.path().join("cache")),
        native: true,
        ..Default::default()
    };

    let (plan, res) = RustAdapter::build(&opt).unwrap();
    assert!(
        res.success,
        "Build should succeed with metadata propagation and aliases: {}",
        res.stderr
    );
    assert!(plan.actions.len() >= 2);
}

#[test]
fn test_m8_alias_and_unaliased_coexist_both_passed_as_extern() {
    let temp_dir = TempDir::new().expect("tempdir");
    let manifest_path = temp_dir.path().join("Cargo.toml");
    fs::write(
        &manifest_path,
        r#"[package]
name = "my-pkg"
version = "0.1.0"
edition = "2021"

[dependencies]
foo = "1.0.0"
foo_alias = { package = "foo", version = "1.0.0" }
"#,
    )
    .unwrap();

    let mut completed = std::collections::HashMap::new();
    let foo_artifact = temp_dir.path().join("libfoo.rlib");
    fs::write(&foo_artifact, b"").unwrap();
    completed.insert("foo@1.0.0".to_string(), foo_artifact.clone());

    let unit = mitts::adapter::rust::types::PlannedCompilationUnit {
        package_name: "my-pkg".to_string(),
        version: "0.1.0".to_string(),
        target_name: "my_pkg".to_string(),
        kind: "lib".to_string(),
        src_path: temp_dir.path().join("src/lib.rs"),
        manifest_path: manifest_path.clone(),
        dependencies: vec!["foo@1.0.0".to_string()],
        ..Default::default()
    };

    let externs = mitts::adapter::rust::dag_unit_externs::resolve_extern_crates_with_target(
        &unit, &completed, false,
    );
    let names: Vec<String> = externs.into_iter().map(|(n, _)| n).collect();
    assert!(
        names.contains(&"foo".to_string()),
        "Must contain unaliased direct dep 'foo': {:?}",
        names
    );
    assert!(
        names.contains(&"foo_alias".to_string()),
        "Must contain aliased dep 'foo_alias': {:?}",
        names
    );
}

#[test]
fn test_m8_multi_version_aliasing_resolves_correct_names() {
    let temp_dir = TempDir::new().expect("tempdir");
    let manifest_path = temp_dir.path().join("Cargo.toml");
    fs::write(
        &manifest_path,
        r#"[package]
name = "my-app"
version = "0.1.0"
edition = "2021"

[dependencies]
http = "1.0.0"
http0 = { package = "http", version = "0.2.0" }
"#,
    )
    .unwrap();

    let mut completed = std::collections::HashMap::new();
    let http02_artifact = temp_dir.path().join("libhttp02.rlib");
    let http10_artifact = temp_dir.path().join("libhttp10.rlib");
    fs::write(&http02_artifact, b"").unwrap();
    fs::write(&http10_artifact, b"").unwrap();
    completed.insert("http@0.2.0".to_string(), http02_artifact.clone());
    completed.insert("http@1.0.0".to_string(), http10_artifact.clone());

    // Unit depending on http@0.2.0
    let unit_dep02 = mitts::adapter::rust::types::PlannedCompilationUnit {
        package_name: "my-app".to_string(),
        version: "0.1.0".to_string(),
        target_name: "my_app".to_string(),
        kind: "bin".to_string(),
        src_path: temp_dir.path().join("src/main.rs"),
        manifest_path: manifest_path.clone(),
        dependencies: vec!["http@0.2.0".to_string()],
        ..Default::default()
    };
    let externs_02 = mitts::adapter::rust::dag_unit_externs::resolve_extern_crates_with_target(
        &unit_dep02,
        &completed,
        false,
    );
    let names_02: Vec<String> = externs_02.into_iter().map(|(n, _)| n).collect();
    assert_eq!(
        names_02,
        vec!["http0"],
        "http@0.2.0 must resolve only to alias http0"
    );

    // Unit depending on http@1.0.0
    let unit_dep10 = mitts::adapter::rust::types::PlannedCompilationUnit {
        package_name: "my-app".to_string(),
        version: "0.1.0".to_string(),
        target_name: "my_app".to_string(),
        kind: "bin".to_string(),
        src_path: temp_dir.path().join("src/main.rs"),
        manifest_path: manifest_path.clone(),
        dependencies: vec!["http@1.0.0".to_string()],
        ..Default::default()
    };
    let externs_10 = mitts::adapter::rust::dag_unit_externs::resolve_extern_crates_with_target(
        &unit_dep10,
        &completed,
        false,
    );
    let names_10: Vec<String> = externs_10.into_iter().map(|(n, _)| n).collect();
    assert_eq!(
        names_10,
        vec!["http"],
        "http@1.0.0 must resolve only to unaliased http"
    );
}

#[test]
fn test_m8_integration_test_named_unit_test_links_package_library() {
    let temp_dir = TempDir::new().expect("tempdir");
    let manifest_path = temp_dir.path().join("Cargo.toml");
    fs::write(
        &manifest_path,
        r#"[package]
name = "parser"
version = "0.1.0"
edition = "2021"
"#,
    )
    .unwrap();

    let mut completed = std::collections::HashMap::new();
    let parser_lib_artifact = temp_dir.path().join("libparser.rlib");
    fs::write(&parser_lib_artifact, b"").unwrap();
    completed.insert("parser@0.1.0".to_string(), parser_lib_artifact.clone());

    // 1. Integration test file tests/parser_unit_test.rs (has _unit_test in name, but src_path != lib.rs)
    let integration_test_unit = mitts::adapter::rust::types::PlannedCompilationUnit {
        package_name: "parser".to_string(),
        version: "0.1.0".to_string(),
        target_name: "parser_unit_test".to_string(),
        kind: "test".to_string(),
        src_path: temp_dir.path().join("tests/parser_unit_test.rs"),
        manifest_path: manifest_path.clone(),
        dependencies: vec!["parser@0.1.0".to_string()],
        ..Default::default()
    };
    let externs = mitts::adapter::rust::dag_unit_externs::resolve_extern_crates_with_target(
        &integration_test_unit,
        &completed,
        false,
    );
    let names: Vec<String> = externs.into_iter().map(|(n, _)| n).collect();
    assert!(
        names.contains(&"parser".to_string()),
        "Integration test tests/parser_unit_test.rs MUST link package library 'parser': {:?}",
        names
    );

    // 2. Unit test of the library itself (src_path == src/lib.rs)
    let lib_unit_test = mitts::adapter::rust::types::PlannedCompilationUnit {
        package_name: "parser".to_string(),
        version: "0.1.0".to_string(),
        target_name: "parser_unit_test".to_string(),
        kind: "test".to_string(),
        src_path: temp_dir.path().join("src/lib.rs"),
        manifest_path: manifest_path.clone(),
        dependencies: vec!["parser@0.1.0".to_string()],
        ..Default::default()
    };
    let externs_lib_test =
        mitts::adapter::rust::dag_unit_externs::resolve_extern_crates_with_target(
            &lib_unit_test,
            &completed,
            false,
        );
    let names_lib_test: Vec<String> = externs_lib_test.into_iter().map(|(n, _)| n).collect();
    assert!(
        !names_lib_test.contains(&"parser".to_string()),
        "Library unit test must NOT circular-link its own library 'parser': {:?}",
        names_lib_test
    );
}

#[test]
fn test_m8_resolve_cargo_empty_parent_filtered() {
    let cargo_bin = std::path::PathBuf::from("cargo");
    let parent = cargo_bin.parent().filter(|p| !p.as_os_str().is_empty());
    assert_eq!(
        parent, None,
        "Bare binary name 'cargo' must filter empty parent"
    );
}
