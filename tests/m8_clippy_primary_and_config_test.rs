/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::rust::inputs::hash_package_base_inputs;
use mitts::adapter::rust::lockfile::{CargoLock, CargoLockPackage};
use mitts::adapter::rust::manifest::CargoToml;
use mitts::adapter::rust::planner_deps::{
    resolve_unit_dependencies, resolve_unit_dependencies_with_manifests,
};
use mitts::adapter::rust::types::{PlannedCompilationUnit, RustBuildOptions};
use mitts::adapter::rust::unit_env::compute_unit_rustc_env;
use mitts::adapter::RustAdapter;
use std::fs;
use tempfile::TempDir;

#[test]
fn test_m8_primary_package_distinction_and_key_isolation() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    // Setup workspace with root Cargo.toml, app member, and core member
    fs::write(
        root.join("Cargo.toml"),
        r#"[workspace]
members = ["app", "core"]
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
core = { path = "../core" }
"#,
    )
    .unwrap();
    fs::write(app_dir.join("src/main.rs"), "fn main() {}\n").unwrap();

    let core_dir = root.join("core");
    fs::create_dir_all(core_dir.join("src")).unwrap();
    fs::write(
        core_dir.join("Cargo.toml"),
        r#"[package]
name = "core"
version = "0.1.0"
edition = "2021"
"#,
    )
    .unwrap();
    fs::write(core_dir.join("src/lib.rs"), "pub fn run() {}\n").unwrap();

    fs::write(
        root.join("Cargo.lock"),
        r#"version = 3

[[package]]
name = "app"
version = "0.1.0"
dependencies = ["core"]

[[package]]
name = "core"
version = "0.1.0"
"#,
    )
    .unwrap();

    // 1. Build plan when app is selected: app is primary, core is dependency
    let opt_app = RustBuildOptions {
        package: Some("app".to_string()),
        manifest_path: Some(root.join("Cargo.toml")),
        ..Default::default()
    };
    let plan_app = RustAdapter::explain_plan(&opt_app).unwrap();
    let app_unit = plan_app
        .units
        .iter()
        .find(|u| u.package_name == "app")
        .unwrap();
    let core_dep_unit = plan_app
        .units
        .iter()
        .find(|u| u.package_name == "core")
        .unwrap();

    let app_env = compute_unit_rustc_env(app_unit, None, None);
    assert_eq!(
        app_env.get("CARGO_PRIMARY_PACKAGE").map(String::as_str),
        Some("1")
    );
    assert!(app_unit.is_primary, "App unit must be primary");

    let core_dep_env = compute_unit_rustc_env(core_dep_unit, None, None);
    assert_eq!(core_dep_env.get("CARGO_PRIMARY_PACKAGE"), None);
    assert!(
        !core_dep_unit.is_primary,
        "Core dep unit must not be primary"
    );

    // 2. Build plan when core is selected: core is primary
    let opt_core = RustBuildOptions {
        package: Some("core".to_string()),
        manifest_path: Some(root.join("Cargo.toml")),
        ..Default::default()
    };
    let plan_core = RustAdapter::explain_plan(&opt_core).unwrap();
    let core_primary_unit = plan_core
        .units
        .iter()
        .find(|u| u.package_name == "core")
        .unwrap();

    let core_primary_env = compute_unit_rustc_env(core_primary_unit, None, None);
    assert_eq!(
        core_primary_env
            .get("CARGO_PRIMARY_PACKAGE")
            .map(String::as_str),
        Some("1")
    );

    // 3. Verify action keys for core differ between primary and non-primary
    let key_ctx_app =
        mitts::adapter::rust::key::derive_unit_action_key_context(&opt_app, &plan_app).unwrap();
    let key_ctx_core =
        mitts::adapter::rust::key::derive_unit_action_key_context(&opt_core, &plan_core).unwrap();

    let core_dep_key = key_ctx_app.compute_dynamic_key(core_dep_unit, &[]);
    let core_prim_key = key_ctx_core.compute_dynamic_key(core_primary_unit, &[]);
    assert_ne!(
        core_dep_key, core_prim_key,
        "Primary and dependency builds of member crate must have distinct action keys"
    );
}

#[test]
fn test_m8_member_clippy_toml_hashing_and_invalidation() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    let member_dir = root.join("my-member");
    fs::create_dir_all(member_dir.join("src")).unwrap();
    fs::write(
        member_dir.join("Cargo.toml"),
        r#"[package]
name = "my-member"
version = "0.1.0"
edition = "2021"
"#,
    )
    .unwrap();
    fs::write(member_dir.join("src/lib.rs"), "pub fn test() {}\n").unwrap();

    // 1. Initial base inputs without clippy.toml
    let base1 = hash_package_base_inputs(&member_dir, None, false).unwrap();
    assert!(!base1.iter().any(|(lbl, _)| lbl.contains("clippy.toml")));

    // 2. Add member-level clippy.toml
    let clippy_p = member_dir.join("clippy.toml");
    fs::write(&clippy_p, "disallowed-names = [\"foo\"]\n").unwrap();

    let base2 = hash_package_base_inputs(&member_dir, None, false).unwrap();
    let clippy_entry1 = base2.iter().find(|(lbl, _)| lbl.contains("clippy.toml"));
    assert!(clippy_entry1.is_some(), "member clippy.toml must be hashed");

    // 3. Edit clippy.toml content -> hash must change
    fs::write(&clippy_p, "disallowed-names = [\"bar\"]\n").unwrap();
    let base3 = hash_package_base_inputs(&member_dir, None, false).unwrap();
    let clippy_entry2 = base3
        .iter()
        .find(|(lbl, _)| lbl.contains("clippy.toml"))
        .unwrap();
    assert_ne!(
        clippy_entry1.unwrap().1,
        clippy_entry2.1,
        "Modified clippy.toml must change hash"
    );
}

#[test]
fn test_m8_library_unit_test_does_not_depend_on_own_rlib() {
    let tmp = TempDir::new().unwrap();
    let pkg_dir = tmp.path().join("my-lib");
    fs::create_dir_all(pkg_dir.join("src")).unwrap();
    fs::write(
        pkg_dir.join("Cargo.toml"),
        r#"[package]
name = "my-lib"
version = "0.1.0"
edition = "2021"
"#,
    )
    .unwrap();
    fs::write(pkg_dir.join("src/lib.rs"), "pub fn hello() {}\n").unwrap();

    let toml = CargoToml::from_file(&pkg_dir.join("Cargo.toml")).unwrap();
    let test_targets = toml.discover_test_targets(&pkg_dir);
    assert_eq!(test_targets.len(), 1);
    assert_eq!(test_targets[0].name, "my_lib_unit_test");

    let lock_pkg = CargoLockPackage {
        name: "my-lib".to_string(),
        version: "0.1.0".to_string(),
        source: None,
        dependencies: Vec::new(),
        checksum: None,
    };
    let lockfile = CargoLock {
        version: 3,
        packages: vec![lock_pkg.clone()],
    };

    let mut units = vec![
        PlannedCompilationUnit {
            package_id: "my-lib@0.1.0".to_string(),
            package_name: "my-lib".to_string(),
            version: "0.1.0".to_string(),
            target_name: "my_lib".to_string(),
            kind: "lib".to_string(),
            src_path: pkg_dir.join("src/lib.rs"),
            manifest_path: pkg_dir.join("Cargo.toml"),
            cache_eligibility: "eligible".to_string(),
            is_workspace: true,
            is_primary: true,
            env_vars: Vec::new(),
            dependencies: Vec::new(),
            edition: Some("2021".to_string()),
            features: Vec::new(),
            for_host: false,
            workspace_root: None,
        },
        PlannedCompilationUnit {
            package_id: "my-lib@0.1.0".to_string(),
            package_name: "my-lib".to_string(),
            version: "0.1.0".to_string(),
            target_name: "my_lib_unit_test".to_string(),
            kind: "test".to_string(),
            src_path: pkg_dir.join("src/lib.rs"),
            manifest_path: pkg_dir.join("Cargo.toml"),
            cache_eligibility: "eligible".to_string(),
            is_workspace: true,
            is_primary: true,
            env_vars: Vec::new(),
            dependencies: Vec::new(),
            edition: Some("2021".to_string()),
            features: Vec::new(),
            for_host: false,
            workspace_root: None,
        },
    ];

    let selected = vec![(&lockfile.packages[0], true)];
    resolve_unit_dependencies(&mut units, &selected, None, std::path::Path::new(".")).unwrap();

    let resolved = vec![(&lockfile.packages[0], true, pkg_dir.clone(), toml)];
    resolve_unit_dependencies_with_manifests(
        &mut units,
        &selected,
        &resolved,
        None,
        std::path::Path::new("."),
    )
    .unwrap();

    let unit_test = units
        .iter()
        .find(|u| u.target_name == "my_lib_unit_test")
        .unwrap();
    assert!(
        !unit_test.dependencies.iter().any(|d| d.contains("my_lib")),
        "Library unit test must not depend on its own library rlib: {:?}",
        unit_test.dependencies
    );
}

#[test]
fn test_m8_external_clippy_toml_invalidates_fast_lock() {
    use mitts::core::build_lock::{
        FastBuildLock, FreshnessCheckArgs, FreshnessResult, RecordStateArgs,
    };

    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let repo_dir = root.join("repo");
    fs::create_dir_all(repo_dir.join("src")).unwrap();
    fs::write(
        repo_dir.join("Cargo.toml"),
        r#"[package]
name = "my-app"
version = "0.1.0"
edition = "2021"
"#,
    )
    .unwrap();
    fs::write(repo_dir.join("src/main.rs"), "fn main() {}\n").unwrap();

    // Place clippy.toml in ancestor directory above repo_dir
    let ancestor_clippy = root.join("clippy.toml");
    fs::write(&ancestor_clippy, "disallowed-names = [\"foo\"]\n").unwrap();

    let record_args = RecordStateArgs {
        project_dir: &repo_dir,
        profile: "debug",
        package: &None,
        features: &[],
        no_default_features: false,
        all_features: false,
        toolchain_version: "rustc 1.97.0",
        action_count: 1,
        root_binary: None,
        outputs: &[],
        stderr: None,
    };
    FastBuildLock::record_state(&record_args).unwrap();

    let check_args = FreshnessCheckArgs {
        project_dir: &repo_dir,
        profile: "debug",
        package: &None,
        features: &[],
        no_default_features: false,
        all_features: false,
        toolchain_version: "rustc 1.97.0",
    };

    // 1. Immediately fresh
    match FastBuildLock::check_freshness(&check_args) {
        FreshnessResult::Fresh { .. } => {}
        FreshnessResult::Stale(r) => panic!("Expected Fresh, got Stale: {}", r),
    }

    // 2. Modify ancestor clippy.toml -> must be Stale
    fs::write(
        &ancestor_clippy,
        "disallowed-names = [\"bar\", \"new_longer_disallowed_name\"]\n",
    )
    .unwrap();
    match FastBuildLock::check_freshness(&check_args) {
        FreshnessResult::Stale(r) => {
            assert!(
                r.contains("Clippy config modified"),
                "Expected Clippy config modified, got: {}",
                r
            );
        }
        FreshnessResult::Fresh { .. } => {
            panic!("Expected Stale after modifying ancestor clippy.toml!")
        }
    }
}

#[test]
fn test_corrupt_dependency_manifest_fails_closed_with_path() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("proj");
    let cache_dir = tmp.path().join("cache");
    let dep_dir = cache_dir.join("sources/broken-dep-0.1.0");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(dep_dir.join("src")).unwrap();
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[dependencies]\nbroken-dep = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(root.join("src/lib.rs"), "pub fn f() {}\n").unwrap();
    fs::write(
        root.join("Cargo.lock"),
        "version = 3\n\n[[package]]\nname = \"app\"\nversion = \"0.1.0\"\ndependencies = [\n \"broken-dep\",\n]\n\n[[package]]\nname = \"broken-dep\"\nversion = \"0.1.0\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\nchecksum = \"0000000000000000000000000000000000000000000000000000000000000000\"\n",
    )
    .unwrap();
    let broken_toml = dep_dir.join("Cargo.toml");
    fs::write(&broken_toml, "[package\nname = \"broken-dep\"\n").unwrap();
    fs::write(dep_dir.join(".mitts-ok"), "").unwrap();
    fs::write(dep_dir.join("src/lib.rs"), "").unwrap();

    let opt = RustBuildOptions {
        manifest_path: Some(root.join("Cargo.toml")),
        cache_dir: Some(cache_dir),
        ..Default::default()
    };
    let err = format!("{:#}", RustAdapter::explain_plan(&opt).unwrap_err());
    assert!(err.contains(&broken_toml.display().to_string()), "{err}");
}
