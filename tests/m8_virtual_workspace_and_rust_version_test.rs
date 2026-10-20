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
use mitts::adapter::rust::types::{PlannedCompilationUnit, RustBuildOptions};
use mitts::adapter::rust::unit_env::compute_unit_rustc_env;
use mitts::adapter::RustAdapter;
use std::fs;
use tempfile::tempdir;

#[test]
fn test_virtual_workspace_all_members_selected() {
    let isolated_cache = tempfile::tempdir().expect("isolated cache dir");
    let tmp = tempdir().unwrap();
    let root = tmp.path();

    let root_toml = r#"
[workspace]
resolver = "2"
members = ["crate-a", "crate-b"]
"#;
    fs::write(root.join("Cargo.toml"), root_toml).unwrap();

    let dir_a = root.join("crate-a");
    fs::create_dir_all(dir_a.join("src")).unwrap();
    let toml_a = r#"
[package]
name = "crate-a"
version = "0.1.0"
edition = "2021"
"#;
    fs::write(dir_a.join("Cargo.toml"), toml_a).unwrap();
    fs::write(dir_a.join("src/lib.rs"), "pub fn hello() -> i32 { 1 }").unwrap();

    let dir_b = root.join("crate-b");
    fs::create_dir_all(dir_b.join("src")).unwrap();
    let toml_b = r#"
[package]
name = "crate-b"
version = "0.2.0"
edition = "2021"

[dependencies]
crate-a = { path = "../crate-a" }
"#;
    fs::write(dir_b.join("Cargo.toml"), toml_b).unwrap();
    fs::write(
        dir_b.join("src/lib.rs"),
        "pub fn run() -> i32 { crate_a::hello() }",
    )
    .unwrap();

    let lock = r#"
version = 3

[[package]]
name = "crate-a"
version = "0.1.0"

[[package]]
name = "crate-b"
version = "0.2.0"
dependencies = ["crate-a"]
"#;
    fs::write(root.join("Cargo.lock"), lock).unwrap();

    let opt = RustBuildOptions {
        cache_dir: Some(isolated_cache.path().to_path_buf()),
        manifest_path: Some(root.join("Cargo.toml")),
        ..Default::default()
    };

    let plan = RustAdapter::explain_plan(&opt).expect("explain plan must succeed");
    let unit_names: Vec<&str> = plan.units.iter().map(|u| u.package_name.as_str()).collect();

    assert!(
        unit_names.contains(&"crate-a"),
        "crate-a must be selected in virtual workspace: {:?}",
        unit_names
    );
    assert!(
        unit_names.contains(&"crate-b"),
        "crate-b must be selected in virtual workspace: {:?}",
        unit_names
    );
}

#[test]
fn test_virtual_workspace_default_members_precedence() {
    let isolated_cache = tempfile::tempdir().expect("isolated cache dir");
    let tmp = tempdir().unwrap();
    let root = tmp.path();

    let root_toml = r#"
[workspace]
resolver = "2"
members = ["crate-a", "crate-b"]
default-members = ["crate-a"]
"#;
    fs::write(root.join("Cargo.toml"), root_toml).unwrap();

    let dir_a = root.join("crate-a");
    fs::create_dir_all(dir_a.join("src")).unwrap();
    let toml_a = r#"
[package]
name = "crate-a"
version = "0.1.0"
edition = "2021"
"#;
    fs::write(dir_a.join("Cargo.toml"), toml_a).unwrap();
    fs::write(dir_a.join("src/lib.rs"), "pub fn a() {}").unwrap();

    let dir_b = root.join("crate-b");
    fs::create_dir_all(dir_b.join("src")).unwrap();
    let toml_b = r#"
[package]
name = "crate-b"
version = "0.2.0"
edition = "2021"
"#;
    fs::write(dir_b.join("Cargo.toml"), toml_b).unwrap();
    fs::write(dir_b.join("src/lib.rs"), "pub fn b() {}").unwrap();

    let lock = r#"
version = 3

[[package]]
name = "crate-a"
version = "0.1.0"

[[package]]
name = "crate-b"
version = "0.2.0"
"#;
    fs::write(root.join("Cargo.lock"), lock).unwrap();

    let opt = RustBuildOptions {
        cache_dir: Some(isolated_cache.path().to_path_buf()),
        manifest_path: Some(root.join("Cargo.toml")),
        ..Default::default()
    };

    let plan = RustAdapter::explain_plan(&opt).expect("explain plan must succeed");
    let unit_names: Vec<&str> = plan.units.iter().map(|u| u.package_name.as_str()).collect();

    assert!(
        unit_names.contains(&"crate-a"),
        "crate-a must be selected: {:?}",
        unit_names
    );
    assert!(
        !unit_names.contains(&"crate-b"),
        "crate-b should not be selected when default-members specifies crate-a: {:?}",
        unit_names
    );
}

#[test]
fn test_cargo_pkg_rust_version_and_primary_package_env() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();

    let toml = r#"
[package]
name = "demo-msrv"
version = "1.2.3"
edition = "2021"
rust-version = "1.71.0"
description = "Demo package for MSRV check"
"#;
    fs::write(root.join("Cargo.toml"), toml).unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/lib.rs"), "").unwrap();

    let unit_ws = PlannedCompilationUnit {
        package_id: "demo-msrv@1.2.3".to_string(),
        package_name: "demo-msrv".to_string(),
        version: "1.2.3".to_string(),
        target_name: "demo_msrv".to_string(),
        kind: "lib".to_string(),
        src_path: root.join("src/lib.rs"),
        manifest_path: root.join("Cargo.toml"),
        cache_eligibility: "Eligible".to_string(),
        is_workspace: true,
        is_primary: true,
        env_vars: Vec::new(),
        dependencies: Vec::new(),
        edition: Some("2021".to_string()),
        features: vec!["default".to_string()],
        for_host: false,
        workspace_root: None,
    };

    let env_ws = compute_unit_rustc_env(&unit_ws, None, None);
    assert_eq!(
        env_ws.get("CARGO_PKG_RUST_VERSION").map(|s| s.as_str()),
        Some("1.71.0")
    );
    assert_eq!(
        env_ws.get("CARGO_PRIMARY_PACKAGE").map(|s| s.as_str()),
        Some("1")
    );

    let unit_non_ws = PlannedCompilationUnit {
        is_workspace: false,
        is_primary: false,
        ..unit_ws
    };
    let env_non_ws = compute_unit_rustc_env(&unit_non_ws, None, None);
    assert_eq!(
        env_non_ws.get("CARGO_PKG_RUST_VERSION").map(|s| s.as_str()),
        Some("1.71.0")
    );
    assert_eq!(env_non_ws.get("CARGO_PRIMARY_PACKAGE"), None);
}

#[test]
fn test_workspace_inherited_rust_version() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();

    let root_toml = r#"
[workspace]
resolver = "2"
members = ["member"]

[workspace.package]
rust-version = "1.75"
"#;
    fs::write(root.join("Cargo.toml"), root_toml).unwrap();

    let member_dir = root.join("member");
    fs::create_dir_all(member_dir.join("src")).unwrap();
    let member_toml = r#"
[package]
name = "member"
version = "0.1.0"
edition = "2021"
rust-version.workspace = true
"#;
    fs::write(member_dir.join("Cargo.toml"), member_toml).unwrap();
    fs::write(member_dir.join("src/lib.rs"), "").unwrap();

    let parsed = CargoToml::from_file(&member_dir.join("Cargo.toml")).unwrap();
    assert_eq!(
        parsed
            .package
            .as_ref()
            .and_then(|p| p.rust_version.as_deref()),
        Some("1.75")
    );
}
