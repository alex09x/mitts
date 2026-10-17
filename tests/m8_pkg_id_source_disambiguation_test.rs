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
use mitts::adapter::rust::package_selection::{
    parse_dep_spec, resolve_lock_dep_candidate, select_transitive_packages,
};
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
        "[package]\nname = \"foo\"\nversion = \"1.0.0\"\nedition = \"2021\"\n[features]\nreg_feat = []\nshared_unused = []\n",
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
        "[package]\nname = \"foo\"\nversion = \"1.0.0\"\nedition = \"2021\"\n[features]\nlocal_feat = []\nshared_unused = []\n",
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
dependencies = ["foo 1.0.0", "foo 1.0.0 (registry+https://github.com/rust-lang/crates.io-index)"]
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
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n[dependencies]\nfoo = \"1.0.0\"\n",
    )
    .unwrap();
    let foo_toml = CargoToml::parse("[package]\nname = \"foo\"\nversion = \"1.0.0\"\n").unwrap();
    let mut ws_manifests = HashMap::new();
    ws_manifests.insert("app".to_string(), (PathBuf::from("."), app_toml.clone()));

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

    let app_deps = vec!["foo 1.0.0".to_string()];
    let make_ctx = |name, version, source, is_ws, toml, deps| PackageFeatureContext {
        name,
        version,
        source,
        is_workspace: is_ws,
        is_root: is_ws,
        toml,
        dependencies: deps,
    };
    let feat_ctxs = vec![
        make_ctx("app", "0.1.0", None, true, &app_toml, &app_deps),
        make_ctx(
            "foo",
            "1.0.0",
            pkg_foo_reg.source.as_deref(),
            false,
            &foo_toml,
            &[][..],
        ),
        make_ctx(
            "foo",
            "1.0.0",
            pkg_foo_git.source.as_deref(),
            false,
            &foo_toml,
            &[][..],
        ),
    ];
    let feat_err = resolve_package_features(&feat_ctxs, &RustBuildOptions::default())
        .expect_err("resolve_package_features must fail closed on ambiguous foo 1.0.0");
    assert!(feat_err.to_string().contains("Ambiguous dependency"));

    let make_unit = |pkg_id: &str, name: &str, ver: &str, is_ws: bool| PlannedCompilationUnit {
        package_id: pkg_id.to_string(),
        package_name: name.to_string(),
        version: ver.to_string(),
        target_name: name.to_string(),
        kind: "lib".to_string(),
        src_path: PathBuf::from("src/lib.rs"),
        manifest_path: PathBuf::from("non_existent_Cargo.toml"),
        cache_eligibility: "eligible".to_string(),
        is_workspace: is_ws,
        is_primary: is_ws,
        env_vars: Vec::new(),
        dependencies: Vec::new(),
        edition: Some("2021".to_string()),
        features: Vec::new(),
        for_host: false,
        workspace_root: None,
    };
    let mut units = vec![
        make_unit("app@0.1.0", "app", "0.1.0", true),
        make_unit(
            "foo@1.0.0 (registry+https://github.com/rust-lang/crates.io-index)",
            "foo",
            "1.0.0",
            false,
        ),
        make_unit(
            "foo@1.0.0 (git+https://github.com/example/foo#111111)",
            "foo",
            "1.0.0",
            false,
        ),
    ];
    let selected = vec![
        (&pkg_app, true),
        (&pkg_foo_reg, false),
        (&pkg_foo_git, false),
    ];
    let dep_err = resolve_unit_dependencies(&mut units, &selected, None, Path::new("."))
        .expect_err("resolve_unit_dependencies must fail closed on ambiguous foo 1.0.0");
    assert!(dep_err.to_string().contains("Ambiguous dependency"));

    assert!(
        resolve_lock_dep_candidate("foo 9.9.9", "foo", Some("9.9.9"), None, &[&pkg_foo_reg])
            .is_err()
    );
}

#[test]
fn test_two_major_versions_of_same_crate_with_distinct_features() {
    let reg = Some("registry+https://github.com/rust-lang/crates.io-index");
    let app_toml = CargoToml::parse(
        r#"[package]
name = "app"
version = "0.1.0"
[features]
default = ["use_v2"]
use_v2 = ["foo2/v2_feat"]
[dependencies]
foo = { version = "1", features = ["v1_feat"] }
foo2 = { package = "foo", version = "2", optional = true }
"#,
    )
    .unwrap();
    let foo1_toml = CargoToml::parse(
        "[package]\nname = \"foo\"\nversion = \"1.0.5\"\n[features]\nv1_feat = []\nv2_feat = []\n",
    )
    .unwrap();
    let foo2_toml = CargoToml::parse(
        "[package]\nname = \"foo\"\nversion = \"2.1.0\"\n[features]\nv1_feat = []\nv2_feat = []\n",
    )
    .unwrap();
    let app_deps = vec![
        "foo 1.0.5 (registry+https://github.com/rust-lang/crates.io-index)".to_string(),
        "foo 2.1.0 (registry+https://github.com/rust-lang/crates.io-index)".to_string(),
    ];
    let make_ctx = |name, version, source, is_ws, toml, deps| PackageFeatureContext {
        name,
        version,
        source,
        is_workspace: is_ws,
        is_root: is_ws,
        toml,
        dependencies: deps,
    };
    let ctxs = vec![
        make_ctx("app", "0.1.0", None, true, &app_toml, &app_deps),
        make_ctx("foo", "1.0.5", reg, false, &foo1_toml, &[][..]),
        make_ctx("foo", "2.1.0", reg, false, &foo2_toml, &[][..]),
    ];
    let resolved = resolve_package_features(&ctxs, &RustBuildOptions::default()).unwrap();
    assert_eq!(
        resolved
            .target
            .get("foo@1.0.5 (registry+https://github.com/rust-lang/crates.io-index)"),
        Some(&vec!["v1_feat".to_string()])
    );
    assert_eq!(
        resolved
            .target
            .get("foo@2.1.0 (registry+https://github.com/rust-lang/crates.io-index)"),
        Some(&vec!["v2_feat".to_string()])
    );
}

#[test]
fn test_broken_path_dependency_in_cargo_toml_reports_missing_path() {
    let temp = TempDir::new().unwrap();
    let proj = temp.path().join("proj");
    fs::create_dir_all(proj.join("src")).unwrap();
    fs::write(
        proj.join("Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[dependencies]\nmissing_dep = { path = \"does-not-exist\" }\n",
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
        "got: {msg}"
    );
}
