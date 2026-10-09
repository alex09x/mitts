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
use std::process::Command;
use tempfile::TempDir;

#[test]
fn test_m8_inactive_optional_dependency_pruned_and_feature_parity_with_cargo() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    // Setup workspace with root app, optional dependency dep_optional, and shared common_dep
    fs::write(
        root.join("Cargo.toml"),
        r#"[workspace]
members = ["app", "dep_optional", "common_dep"]
resolver = "2"
"#,
    )
    .unwrap();

    let app_dir = root.join("app");
    fs::create_dir_all(app_dir.join("src")).unwrap();
    fs::write(
        app_dir.join("Cargo.toml"),
        r#"[package]
name = "app"
version = "0.1.0"
edition = "2021"

[dependencies]
dep_optional = { path = "../dep_optional", optional = true }
common_dep = { path = "../common_dep" }

[features]
net = ["dep:dep_optional"]
"#,
    )
    .unwrap();
    fs::write(app_dir.join("src/lib.rs"), "pub fn app_run() {}\n").unwrap();

    let opt_dir = root.join("dep_optional");
    fs::create_dir_all(opt_dir.join("src")).unwrap();
    fs::write(
        opt_dir.join("Cargo.toml"),
        r#"[package]
name = "dep_optional"
version = "0.1.0"
edition = "2021"

[dependencies]
common_dep = { path = "../common_dep", features = ["extra_feat"] }
"#,
    )
    .unwrap();
    fs::write(opt_dir.join("src/lib.rs"), "pub fn opt_run() {}\n").unwrap();

    let common_dir = root.join("common_dep");
    fs::create_dir_all(common_dir.join("src")).unwrap();
    fs::write(
        common_dir.join("Cargo.toml"),
        r#"[package]
name = "common_dep"
version = "0.1.0"
edition = "2021"

[features]
extra_feat = []
"#,
    )
    .unwrap();
    fs::write(common_dir.join("src/lib.rs"), "pub fn common_run() {}\n").unwrap();

    // Generate Cargo.lock
    let lock_status = Command::new("cargo")
        .args(["generate-lockfile"])
        .current_dir(root)
        .status();
    if let Ok(st) = lock_status {
        assert!(st.success(), "cargo generate-lockfile failed");
    }

    // --- Scenario 1: Without 'net' feature ---
    // Inactive optional dependency `dep_optional` must NOT be in the plan.
    // Common dependency `common_dep` must NOT have `extra_feat` enabled.
    let opt_without_net = RustBuildOptions {
        manifest_path: Some(app_dir.join("Cargo.toml")),
        ..Default::default()
    };
    let plan_without_net = RustAdapter::explain_plan(&opt_without_net)
        .expect("explain_plan without net feature failed");

    let planned_pkg_names: Vec<String> = plan_without_net
        .units
        .iter()
        .map(|u| u.package_name.clone())
        .collect();
    assert!(
        !planned_pkg_names.contains(&"dep_optional".to_string()),
        "dep_optional must NOT be in compilation plan when 'net' feature is inactive; got {:?}",
        planned_pkg_names
    );
    assert!(
        planned_pkg_names.contains(&"app".to_string()),
        "app must be in plan"
    );
    assert!(
        planned_pkg_names.contains(&"common_dep".to_string()),
        "common_dep must be in plan"
    );

    let common_unit_without_net = plan_without_net
        .units
        .iter()
        .find(|u| u.package_name == "common_dep")
        .expect("common_dep unit not found");
    assert!(
        !common_unit_without_net.features.contains(&"extra_feat".to_string()),
        "common_dep must NOT receive 'extra_feat' from inactive optional dependency; got features: {:?}",
        common_unit_without_net.features
    );

    // Verify against Cargo's ground truth via message-format=json
    let cargo_check_without = Command::new("cargo")
        .args(["check", "--message-format=json"])
        .current_dir(&app_dir)
        .output();
    if let Ok(c_out) = cargo_check_without {
        if c_out.status.success() {
            let stderr_str = String::from_utf8_lossy(&c_out.stderr);
            assert!(
                !stderr_str.contains("dep_optional"),
                "Cargo must not compile dep_optional without net feature"
            );
        }
    }

    // --- Scenario 2: With 'net' feature enabled ---
    // Optional dependency `dep_optional` IS activated.
    // Common dependency `common_dep` MUST receive `extra_feat`.
    let opt_with_net = RustBuildOptions {
        manifest_path: Some(app_dir.join("Cargo.toml")),
        features: vec!["net".to_string()],
        ..Default::default()
    };
    let plan_with_net =
        RustAdapter::explain_plan(&opt_with_net).expect("explain_plan with net feature failed");

    let planned_pkg_names_with_net: Vec<String> = plan_with_net
        .units
        .iter()
        .map(|u| u.package_name.clone())
        .collect();
    assert!(
        planned_pkg_names_with_net.contains(&"dep_optional".to_string()),
        "dep_optional MUST be in compilation plan when 'net' feature is active; got {:?}",
        planned_pkg_names_with_net
    );

    let common_unit_with_net = plan_with_net
        .units
        .iter()
        .find(|u| u.package_name == "common_dep")
        .expect("common_dep unit not found");
    assert!(
        common_unit_with_net
            .features
            .contains(&"extra_feat".to_string()),
        "common_dep MUST receive 'extra_feat' when dep_optional is activated; got features: {:?}",
        common_unit_with_net.features
    );
}

#[test]
fn test_m8_target_dependency_optional_feature_merging_and_dev_dep_isolation() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    fs::write(
        root.join("Cargo.toml"),
        r#"[workspace]
members = ["app", "sub_dep"]
resolver = "2"
"#,
    )
    .unwrap();

    let app_dir = root.join("app");
    fs::create_dir_all(app_dir.join("src")).unwrap();
    fs::write(
        app_dir.join("Cargo.toml"),
        r#"[package]
name = "app"
version = "0.1.0"
edition = "2021"

[features]
net = ["sub_dep"]

[target.'cfg(all())'.dependencies]
sub_dep = { path = "../sub_dep", optional = true, features = ["feat_from_target"] }

[target.'cfg(all())'.dev-dependencies]
sub_dep = { path = "../sub_dep" }
"#,
    )
    .unwrap();
    fs::write(app_dir.join("src/lib.rs"), "pub fn app_run() {}\n").unwrap();

    let sub_dir = root.join("sub_dep");
    fs::create_dir_all(sub_dir.join("src")).unwrap();
    fs::write(
        sub_dir.join("Cargo.toml"),
        r#"[package]
name = "sub_dep"
version = "0.1.0"
edition = "2021"

[features]
feat_from_target = []
"#,
    )
    .unwrap();
    fs::write(sub_dir.join("src/lib.rs"), "pub fn sub_run() {}\n").unwrap();

    let lock_status = Command::new("cargo")
        .args(["generate-lockfile"])
        .current_dir(root)
        .status();
    if let Ok(st) = lock_status {
        assert!(st.success(), "cargo generate-lockfile failed");
    }

    // 1. Without 'net': sub_dep must NOT be in plan
    let opt_without = RustBuildOptions {
        manifest_path: Some(app_dir.join("Cargo.toml")),
        ..Default::default()
    };
    let plan_without =
        RustAdapter::explain_plan(&opt_without).expect("explain_plan without net failed");
    let pkgs_without: Vec<String> = plan_without
        .units
        .iter()
        .map(|u| u.package_name.clone())
        .collect();
    assert!(
        !pkgs_without.contains(&"sub_dep".to_string()),
        "sub_dep must not be in compilation plan when optional and inactive"
    );

    // 2. With 'net': sub_dep MUST be in plan AND have 'feat_from_target'
    let opt_with = RustBuildOptions {
        manifest_path: Some(app_dir.join("Cargo.toml")),
        features: vec!["net".to_string()],
        ..Default::default()
    };
    let plan_with = RustAdapter::explain_plan(&opt_with).expect("explain_plan with net failed");
    let sub_unit = plan_with
        .units
        .iter()
        .find(|u| u.package_name == "sub_dep")
        .expect("sub_dep unit must be present in compilation plan");
    assert!(
        sub_unit.features.contains(&"feat_from_target".to_string()),
        "sub_dep must have 'feat_from_target' activated from target dependencies; got {:?}",
        sub_unit.features
    );
}

