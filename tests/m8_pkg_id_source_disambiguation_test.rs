/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::rust::dag_unit_externs::resolve_extern_crates;
use mitts::adapter::rust::features::{resolve_package_features, PackageFeatureContext};
use mitts::adapter::rust::lockfile::{CargoLock, CargoLockPackage};
use mitts::adapter::rust::manifest::CargoToml;
use mitts::adapter::rust::package_selection::{parse_dep_spec, select_transitive_packages};
use mitts::adapter::rust::planner_deps::resolve_unit_dependencies;
use mitts::adapter::rust::types::{PlannedCompilationUnit, RustBuildOptions};
use mitts::adapter::RustAdapter;
use mitts::core::source_store::MITTS_OK_MARKER;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

#[test]
fn test_parse_dep_spec_all_three_cargo_lock_forms() {
    assert_eq!(parse_dep_spec("foo"), ("foo", None, None));
    assert_eq!(parse_dep_spec("foo 1.0.0"), ("foo", Some("1.0.0"), None));
    assert_eq!(
        parse_dep_spec("foo 1.0.0 (registry+https://github.com/rust-lang/crates.io-index)"),
        (
            "foo",
            Some("1.0.0"),
            Some("registry+https://github.com/rust-lang/crates.io-index")
        )
    );
    assert_eq!(
        parse_dep_spec("foo 1.0.0 (git+https://github.com/example/foo?branch=main#abcdef12)"),
        (
            "foo",
            Some("1.0.0"),
            Some("git+https://github.com/example/foo?branch=main#abcdef12")
        )
    );
}

#[test]
fn test_two_foo_1_0_0_packages_path_fork_and_crates_io_isolated_features_and_action_keys() {
    let temp = TempDir::new().unwrap();
    let mitts_cache = temp.path().join("mitts_cache");

    // Populate registry `foo 1.0.0` in SourceStore cache
    let reg_foo_dir = mitts_cache.join("sources/foo-1.0.0");
    fs::create_dir_all(reg_foo_dir.join("src")).unwrap();
    fs::write(
        reg_foo_dir.join("Cargo.toml"),
        r#"[package]
name = "foo"
version = "1.0.0"
edition = "2021"

[features]
reg_feat = []
shared_unused = []
"#,
    )
    .unwrap();
    fs::write(
        reg_foo_dir.join("src/lib.rs"),
        "pub fn from_registry() -> u32 { 2 }\n",
    )
    .unwrap();
    fs::write(reg_foo_dir.join(MITTS_OK_MARKER), "ok\n").unwrap();

    // Populate local path fork `foo 1.0.0` (`source = None`)
    let proj = temp.path().join("proj");
    let foo_fork = proj.join("foo-fork");
    fs::create_dir_all(proj.join("src")).unwrap();
    fs::create_dir_all(foo_fork.join("src")).unwrap();
    fs::write(
        foo_fork.join("Cargo.toml"),
        r#"[package]
name = "foo"
version = "1.0.0"
edition = "2021"

[features]
local_feat = []
shared_unused = []
"#,
    )
    .unwrap();
    fs::write(
        foo_fork.join("src/lib.rs"),
        "pub fn from_local_fork() -> u32 { 1 }\n",
    )
    .unwrap();

    // `app` depends on both `foo 1.0.0` (path fork) and `foo 1.0.0` (crates.io) with distinct features
    fs::write(
        proj.join("Cargo.toml"),
        r#"[package]
name = "app"
version = "0.1.0"
edition = "2021"

[dependencies]
foo_local = { package = "foo", path = "foo-fork", features = ["local_feat"] }
foo_reg = { package = "foo", version = "1.0.0", features = ["reg_feat"] }
"#,
    )
    .unwrap();
    fs::write(proj.join("src/lib.rs"), "pub fn run() {}\n").unwrap();
    fs::write(
        proj.join("Cargo.lock"),
        r#"version = 3

[[package]]
name = "app"
version = "0.1.0"
dependencies = [
 "foo 1.0.0",
 "foo 1.0.0 (registry+https://github.com/rust-lang/crates.io-index)",
]

[[package]]
name = "foo"
version = "1.0.0"

[[package]]
name = "foo"
version = "1.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
"#,
    )
    .unwrap();

    let opt = RustBuildOptions {
        manifest_path: Some(proj.join("Cargo.toml")),
        cache_dir: Some(mitts_cache.clone()),
        ..Default::default()
    };
    let plan = RustAdapter::explain_plan(&opt).expect("explain_plan must succeed");

    let local_id = "foo@1.0.0::lib:foo";
    let reg_id = "foo@1.0.0 (registry+https://github.com/rust-lang/crates.io-index)::lib:foo";

    let foo_local_unit = plan
        .units
        .iter()
        .find(|u| u.unit_id() == local_id)
        .expect("local path foo@1.0.0 unit must be present");
    let foo_reg_unit = plan
        .units
        .iter()
        .find(|u| u.unit_id() == reg_id)
        .expect("registry foo@1.0.0 unit must be present");

    assert_eq!(foo_local_unit.manifest_path, foo_fork.join("Cargo.toml"));
    assert_eq!(foo_reg_unit.manifest_path, reg_foo_dir.join("Cargo.toml"));

    // Features must NOT merge between the two `foo 1.0.0` packages
    assert_eq!(foo_local_unit.features, vec!["local_feat".to_string()]);
    assert_eq!(foo_reg_unit.features, vec!["reg_feat".to_string()]);

    // `app` must depend on both distinct `foo` units
    let app_unit = plan
        .units
        .iter()
        .find(|u| u.package_name == "app")
        .expect("app unit");
    assert!(app_unit.dependencies.contains(&local_id.to_string()));
    assert!(app_unit.dependencies.contains(&reg_id.to_string()));

    // Derived action keys must be present and distinct
    let keys = RustAdapter::derive_unit_action_keys(&opt, &plan)
        .expect("derive_unit_action_keys must succeed");
    let key_local = keys.get(local_id).expect("action key for local foo");
    let key_reg = keys.get(reg_id).expect("action key for registry foo");
    assert_ne!(
        key_local, key_reg,
        "Action keys for path-fork foo 1.0.0 and registry foo 1.0.0 must differ"
    );

    // Extern crate alias resolution must bind foo_local and foo_reg to their respective artifacts
    let mut completed = HashMap::new();
    let local_rlib = PathBuf::from("/tmp/libfoo_local.rlib");
    let reg_rlib = PathBuf::from("/tmp/libfoo_reg.rlib");
    completed.insert(local_id.to_string(), local_rlib.clone());
    completed.insert(reg_id.to_string(), reg_rlib.clone());
    let externs: HashMap<String, PathBuf> = resolve_extern_crates(app_unit, &completed)
        .into_iter()
        .collect();
    assert_eq!(externs.get("foo_local"), Some(&local_rlib));
    assert_eq!(externs.get("foo_reg"), Some(&reg_rlib));
}

#[test]
fn test_ambiguous_dep_spec_fails_closed_across_selection_features_and_deps() {
    let pkg_app = CargoLockPackage {
        name: "app".to_string(),
        version: "0.1.0".to_string(),
        source: None,
        checksum: None,
        // Ambiguous: "foo 1.0.0" without source when two sourced foo 1.0.0 packages exist
        dependencies: vec!["foo 1.0.0".to_string()],
    };
    let pkg_foo_reg = CargoLockPackage {
        name: "foo".to_string(),
        version: "1.0.0".to_string(),
        source: Some("registry+https://github.com/rust-lang/crates.io-index".to_string()),
        checksum: None,
        dependencies: Vec::new(),
    };
    let pkg_foo_git = CargoLockPackage {
        name: "foo".to_string(),
        version: "1.0.0".to_string(),
        source: Some("git+https://github.com/example/foo#111111".to_string()),
        checksum: None,
        dependencies: Vec::new(),
    };

    let lockfile = CargoLock {
        version: 3,
        packages: vec![pkg_app.clone(), pkg_foo_reg.clone(), pkg_foo_git.clone()],
    };

    let app_toml = CargoToml::parse(
        r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
foo = "1.0.0"
"#,
    )
    .unwrap();
    let foo_toml = CargoToml::parse(
        r#"[package]
name = "foo"
version = "1.0.0"
"#,
    )
    .unwrap();

    let mut ws_manifests = HashMap::new();
    ws_manifests.insert("app".to_string(), (PathBuf::from("."), app_toml.clone()));

    // 1. select_transitive_packages must fail closed on ambiguous spec
    let sel_err = select_transitive_packages(
        &lockfile,
        &ws_manifests,
        &app_toml,
        Path::new("."),
        &RustBuildOptions::default(),
    )
    .expect_err("select_transitive_packages must fail closed on ambiguous foo 1.0.0");
    assert!(
        sel_err.to_string().contains("Ambiguous dependency"),
        "unexpected error: {sel_err}"
    );

    // 2. resolve_package_features must fail closed on ambiguous spec
    let app_deps = vec!["foo 1.0.0".to_string()];
    let feat_ctxs = vec![
        PackageFeatureContext {
            name: "app",
            version: "0.1.0",
            source: None,
            is_workspace: true,
            is_root: true,
            toml: &app_toml,
            dependencies: &app_deps,
        },
        PackageFeatureContext {
            name: "foo",
            version: "1.0.0",
            source: pkg_foo_reg.source.as_deref(),
            is_workspace: false,
            is_root: false,
            toml: &foo_toml,
            dependencies: &[],
        },
        PackageFeatureContext {
            name: "foo",
            version: "1.0.0",
            source: pkg_foo_git.source.as_deref(),
            is_workspace: false,
            is_root: false,
            toml: &foo_toml,
            dependencies: &[],
        },
    ];
    let feat_err = resolve_package_features(&feat_ctxs, &RustBuildOptions::default())
        .expect_err("resolve_package_features must fail closed on ambiguous foo 1.0.0");
    assert!(
        feat_err.to_string().contains("Ambiguous dependency"),
        "unexpected error: {feat_err}"
    );

    // 3. resolve_unit_dependencies must fail closed on ambiguous library units
    let make_lib_unit = |pkg_id: &str| PlannedCompilationUnit {
        package_id: pkg_id.to_string(),
        package_name: "foo".to_string(),
        version: "1.0.0".to_string(),
        target_name: "foo".to_string(),
        kind: "lib".to_string(),
        src_path: PathBuf::from("src/lib.rs"),
        manifest_path: PathBuf::from("Cargo.toml"),
        cache_eligibility: "eligible".to_string(),
        is_workspace: false,
        is_primary: false,
        env_vars: Vec::new(),
        dependencies: Vec::new(),
        edition: Some("2021".to_string()),
        features: Vec::new(),
        for_host: false,
        workspace_root: None,
    };
    let mut units = vec![
        PlannedCompilationUnit {
            package_id: "app@0.1.0".to_string(),
            package_name: "app".to_string(),
            version: "0.1.0".to_string(),
            target_name: "app".to_string(),
            kind: "lib".to_string(),
            src_path: PathBuf::from("src/lib.rs"),
            manifest_path: PathBuf::from(" non_existent_Cargo.toml"),
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
        make_lib_unit("foo@1.0.0 (registry+https://github.com/rust-lang/crates.io-index)"),
        make_lib_unit("foo@1.0.0 (git+https://github.com/example/foo#111111)"),
    ];
    let selected = vec![
        (&pkg_app, true),
        (&pkg_foo_reg, false),
        (&pkg_foo_git, false),
    ];
    let dep_err = resolve_unit_dependencies(&mut units, &selected, None, Path::new("."))
        .expect_err("resolve_unit_dependencies must fail closed on ambiguous foo 1.0.0");
    assert!(
        dep_err.to_string().contains("Ambiguous unit dependency"),
        "unexpected error: {dep_err}"
    );
}

#[test]
fn test_broken_path_dependency_in_cargo_toml_reports_missing_path() {
    let temp = TempDir::new().unwrap();
    let proj = temp.path().join("proj");
    fs::create_dir_all(proj.join("src")).unwrap();
    fs::write(
        proj.join("Cargo.toml"),
        r#"[package]
name = "app"
version = "0.1.0"
edition = "2021"

[dependencies]
missing_dep = { path = "does-not-exist" }
"#,
    )
    .unwrap();
    fs::write(proj.join("src/lib.rs"), "pub fn f() {}\n").unwrap();

    let err = RustAdapter::explain_plan(&RustBuildOptions {
        manifest_path: Some(proj.join("Cargo.toml")),
        ..Default::default()
    })
    .expect_err("broken path dependency must fail closed");
    let msg = format!("{:#}", err);
    assert!(
        msg.contains("failed to read") && msg.contains("does-not-exist"),
        "Expected missing manifest path in error, got: {msg}"
    );
}
