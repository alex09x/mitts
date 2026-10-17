/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::rust::compiler::RustcInvocation;
use mitts::adapter::rust::features::{resolve_package_features, PackageFeatureContext};
use mitts::adapter::rust::manifest::CargoToml;
use mitts::adapter::rust::source_lookup::find_crate_source_dir;
use mitts::adapter::rust::types::{BuildMode, PlannedCompilationUnit, RustBuildOptions};
use std::fs;

#[test]
fn test_m8_features_resolution_complex() {
    let toml_str = r#"
[package]
name = "my-crate"
version = "0.1.0"
edition = "2021"

[dependencies]
foo_alias = { package = "foo", version = "1.0", optional = true }
bar = { version = "1.0", optional = true }

[features]
default = ["std"]
std = ["dep:foo_alias", "bar?/extra"]
extra_feat = ["dep:bar"]
"#;

    let toml = CargoToml::parse(toml_str).expect("parse manifest");
    let dep_specs = vec!["foo@1.0.0".to_string(), "bar@1.0.0".to_string()];

    let pkg_ctx = PackageFeatureContext {
        name: "my-crate",
        version: "0.1.0",
        is_workspace: true,
        is_root: true,
        toml: &toml,
        dependencies: &dep_specs,
    };

    let pkgs = vec![pkg_ctx];

    // 1. With default features
    let opt_default = RustBuildOptions::default();
    let feats = resolve_package_features(&pkgs, &opt_default).unwrap();
    let resolved = feats.get("my-crate@0.1.0").expect("crate features");
    assert!(resolved.contains(&"default".to_string()));
    assert!(resolved.contains(&"std".to_string()));

    // 2. With all-features
    let opt_all = RustBuildOptions {
        all_features: true,
        ..Default::default()
    };
    let feats_all = resolve_package_features(&pkgs, &opt_all).unwrap();
    let resolved_all = feats_all.get("my-crate@0.1.0").expect("crate features");
    assert!(resolved_all.contains(&"extra_feat".to_string()));

    // 3. With no-default-features
    let opt_no_def = RustBuildOptions {
        no_default_features: true,
        ..Default::default()
    };
    let feats_no_def = resolve_package_features(&pkgs, &opt_no_def).unwrap();
    assert!(feats_no_def
        .get("my-crate@0.1.0")
        .map(|f| f.is_empty())
        .unwrap_or(true));
}

#[test]
fn test_m8_source_lookup_cache_and_nested_git() {
    let temp_home = tempfile::tempdir().expect("temp home");

    // 1. ~/.cache/mitts/sources
    let mitts_src = temp_home
        .path()
        .join(".cache")
        .join("mitts")
        .join("sources")
        .join("cached-pkg-1.0.0");
    fs::create_dir_all(&mitts_src).unwrap();
    fs::write(
        mitts_src.join("Cargo.toml"),
        "[package]\nname = \"cached-pkg\"\nversion = \"1.0.0\"\n",
    )
    .unwrap();
    fs::write(mitts_src.join(".mitts-ok"), "ok\n").unwrap();

    // 2. git repo with crates/nested
    let git_nested = temp_home
        .path()
        .join(".cargo")
        .join("git")
        .join("checkouts")
        .join("monorepo-12345")
        .join("11223344")
        .join("crates")
        .join("nested-crate");
    fs::create_dir_all(&git_nested).unwrap();
    fs::write(
        git_nested.join("Cargo.toml"),
        "[package]\nname = \"nested-crate\"\nversion = \"0.2.0\"\n",
    )
    .unwrap();

    let orig_home = std::env::var("HOME").ok();
    std::env::set_var("HOME", temp_home.path());

    let found_cached = find_crate_source_dir("cached-pkg", "1.0.0", None);
    assert_eq!(found_cached, Some(mitts_src));

    let found_nested = find_crate_source_dir(
        "nested-crate",
        "0.2.0",
        Some("git+https://github.com/test/mono#11223344"),
    );
    assert_eq!(found_nested, Some(git_nested));

    if let Some(h) = orig_home {
        std::env::set_var("HOME", h);
    }
}

#[test]
fn test_m8_compiler_invocation_execution() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let out_dir = temp_dir.path().join("out");
    fs::create_dir_all(&out_dir).unwrap();

    let src_file = temp_dir.path().join("lib.rs");
    fs::write(&src_file, "pub fn add(a: i32, b: i32) -> i32 { a + b }\n").unwrap();

    let unit = PlannedCompilationUnit {
        package_id: "test_lib 0.1.0".to_string(),
        package_name: "test_lib".to_string(),
        version: "0.1.0".to_string(),
        target_name: "test_lib".to_string(),
        kind: "lib".to_string(),
        src_path: src_file,
        edition: Some("2021".to_string()),
        is_workspace: true,
        is_primary: false,
        manifest_path: temp_dir.path().join("Cargo.toml"),
        cache_eligibility: "eligible".to_string(),
        env_vars: Vec::new(),
        dependencies: Vec::new(),
        features: Vec::new(),
        for_host: false,
        workspace_root: None,
    };

    let empty_feats = Vec::new();
    let empty_externs = Vec::new();
    let empty_args = Vec::new();

    let inv = RustcInvocation {
        unit: &unit,
        action_key: "0123456789abcdef0123456789abcdef",
        out_dir: &out_dir,
        dest_binary: None,
        extern_crates: &empty_externs,
        release: false,
        features: &empty_feats,
        directives: None,
        mode: BuildMode::Build,
        extra_compiler_args: &empty_args,
        jobserver_auth: None,
        deps_dir: None,
        target_dir: None,
    };

    let res = inv.execute().expect("execute rustc");
    assert!(
        res.success,
        "Compilation of simple lib must succeed: {}",
        res.stderr
    );
    assert_eq!(res.exit_code, 0);
    assert!(res.primary_output.is_some());
}

#[test]
fn test_m8_features_dual_compiled_host_and_target() {
    let root_str = r#"
[package]
name = "root"
version = "0.1.0"
edition = "2021"

[dependencies]
worker = { version = "1.0", default-features = false }
"#;
    let worker_str = r#"
[package]
name = "worker"
version = "1.0.0"
edition = "2021"

[dependencies]
dual_crate = { version = "2.0", default-features = false }

[build-dependencies]
dual_crate = { version = "2.0", default-features = false, features = ["build_mode"] }
"#;
    let dual_str = r#"
[package]
name = "dual_crate"
version = "2.0.0"
edition = "2021"

[features]
build_mode = []
"#;

    let root_toml = CargoToml::parse(root_str).unwrap();
    let worker_toml = CargoToml::parse(worker_str).unwrap();
    let dual_toml = CargoToml::parse(dual_str).unwrap();

    let root_deps = vec!["worker 1.0.0".to_string()];
    let worker_deps = vec!["dual_crate 2.0.0".to_string()];
    let dual_deps = Vec::new();

    let pkgs = vec![
        PackageFeatureContext {
            name: "root",
            version: "0.1.0",
            is_workspace: true,
            is_root: true,
            toml: &root_toml,
            dependencies: &root_deps,
        },
        PackageFeatureContext {
            name: "worker",
            version: "1.0.0",
            is_workspace: false,
            is_root: false,
            toml: &worker_toml,
            dependencies: &worker_deps,
        },
        PackageFeatureContext {
            name: "dual_crate",
            version: "2.0.0",
            is_workspace: false,
            is_root: false,
            toml: &dual_toml,
            dependencies: &dual_deps,
        },
    ];

    let opt = RustBuildOptions::default();
    let resolved = resolve_package_features(&pkgs, &opt).unwrap();

    assert!(
        resolved.active_target_packages.contains("worker@1.0.0"),
        "worker must be in target"
    );
    assert!(
        resolved.active_target_packages.contains("dual_crate@2.0.0"),
        "dual_crate must be in target"
    );
    assert!(
        resolved.active_host_packages.contains("dual_crate@2.0.0"),
        "dual_crate must be in host"
    );
}
