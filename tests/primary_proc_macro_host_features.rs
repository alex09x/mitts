/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::rust::features::{
    resolve_package_features_with_resolver, PackageFeatureContext,
};
use mitts::adapter::rust::manifest::CargoToml;
use mitts::adapter::{BuildMode, RustAdapter, RustBuildOptions};
use std::fs;
use std::path::Path;
use tempfile::TempDir;

fn write_crate(dir: &Path, manifest: &str, lib_src: &str) {
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("Cargo.toml"), manifest).unwrap();
    fs::write(dir.join("src/lib.rs"), lib_src).unwrap();
}

#[test]
fn test_standalone_root_proc_macro_host_semantics_and_feature_survival() {
    let isolated_cache = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();

    write_crate(
        &root.path().join("helper"),
        "[package]\nname = \"helper\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        "pub fn helper_fn() -> i32 { 10 }\n",
    );
    write_crate(
        &root.path().join("test_helper"),
        "[package]\nname = \"test_helper\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        "pub fn test_val() -> i32 { 20 }\n",
    );

    let macro_dir = root.path().join("my_macro");
    write_crate(
        &macro_dir,
        "[package]\nname = \"my_macro\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
         [lib]\nproc-macro = true\n\n\
         [features]\ndefault = [\"std\"]\nstd = [\"dep:helper\"]\nextra = []\n\n\
         [dependencies]\nhelper = { path = \"../helper\", optional = true }\n\n\
         [dev-dependencies]\ntest_helper = { path = \"../test_helper\" }\n",
        "extern crate proc_macro;\nuse proc_macro::TokenStream;\n\
         #[proc_macro]\npub fn identity(item: TokenStream) -> TokenStream { item }\n\
         #[cfg(feature = \"std\")]\nconst _: fn() -> i32 = helper::helper_fn;\n\
         #[test]\nfn test_in_macro() { assert_eq!(test_helper::test_val(), 20); }\n",
    );

    let manifest_path = macro_dir.join("Cargo.toml");
    let target_dir = macro_dir.join("target");

    // 1. Build mode plan: proc-macro is for_host, helper is for_host, test_helper not in plan
    let opt_build = RustBuildOptions {
        manifest_path: Some(manifest_path.clone()),
        cache_dir: Some(isolated_cache.path().to_path_buf()),
        target_dir: Some(target_dir.clone()),
        ..Default::default()
    };
    let plan_build = RustAdapter::explain_plan(&opt_build).expect("explain plan build");
    let macro_unit = plan_build
        .units
        .iter()
        .find(|u| u.package_name == "my_macro" && u.kind == "proc-macro")
        .expect("my_macro proc-macro unit");
    assert!(macro_unit.for_host, "root proc-macro must be for_host");
    assert!(
        macro_unit.features.contains(&"default".to_string()),
        "default feature must survive on host proc-macro: {:?}",
        macro_unit.features
    );
    assert!(
        macro_unit.features.contains(&"std".to_string()),
        "std feature must survive on host proc-macro: {:?}",
        macro_unit.features
    );

    let helper_unit = plan_build
        .units
        .iter()
        .find(|u| u.package_name == "helper")
        .expect("helper unit");
    assert!(
        helper_unit.for_host,
        "proc-macro dependency must be for_host"
    );
    assert!(
        !plan_build
            .units
            .iter()
            .any(|u| u.package_name == "test_helper"),
        "test_helper must not be in build mode plan"
    );

    // 2. Test mode plan: unit test has for_host=true, test_helper dev-dependency is active and for_host
    let opt_test = RustBuildOptions {
        manifest_path: Some(manifest_path.clone()),
        cache_dir: Some(isolated_cache.path().to_path_buf()),
        target_dir: Some(target_dir.clone()),
        mode: BuildMode::Test,
        all_tests: true,
        ..Default::default()
    };
    let plan_test = RustAdapter::explain_plan(&opt_test).expect("explain plan test");
    let test_unit = plan_test
        .units
        .iter()
        .find(|u| u.package_name == "my_macro" && u.kind == "test")
        .expect("my_macro test unit");
    assert!(test_unit.for_host, "proc-macro test unit must be for_host");
    assert!(
        test_unit.features.contains(&"default".to_string()),
        "default feature must survive on proc-macro test unit: {:?}",
        test_unit.features
    );

    let test_helper_unit = plan_test
        .units
        .iter()
        .find(|u| u.package_name == "test_helper")
        .expect("test_helper unit in test mode");
    assert!(
        test_helper_unit.for_host,
        "dev-dependency of proc-macro must be for_host"
    );

    // 3. Feature selection: --no-default-features and --features extra
    let opt_no_def = RustBuildOptions {
        manifest_path: Some(manifest_path.clone()),
        cache_dir: Some(isolated_cache.path().to_path_buf()),
        target_dir: Some(target_dir.clone()),
        no_default_features: true,
        features: vec!["extra".to_string()],
        ..Default::default()
    };
    let plan_no_def = RustAdapter::explain_plan(&opt_no_def).expect("explain plan no_def");
    let macro_no_def = plan_no_def
        .units
        .iter()
        .find(|u| u.package_name == "my_macro" && u.kind == "proc-macro")
        .expect("my_macro unit");
    assert!(
        !macro_no_def.features.contains(&"default".to_string()),
        "default feature must be suppressed"
    );
    assert!(
        macro_no_def.features.contains(&"extra".to_string()),
        "explicit extra feature must be present"
    );

    let opt_explicit_test = RustBuildOptions {
        mode: BuildMode::Test,
        all_tests: true,
        ..opt_no_def
    };
    let explicit_test_plan = RustAdapter::explain_plan(&opt_explicit_test).unwrap();
    let explicit_test = explicit_test_plan
        .units
        .iter()
        .find(|unit| unit.package_name == "my_macro" && unit.kind == "test")
        .unwrap();
    assert!(explicit_test.for_host);
    assert!(explicit_test.features.contains(&"extra".to_string()));
    assert!(!explicit_test.features.contains(&"default".to_string()));

    // 4. Actual compilation execution for build and test modes
    let (_, res_build) = RustAdapter::build(&opt_build).expect("build proc macro");
    assert!(
        res_build.success,
        "Standalone proc-macro build failed: {}",
        res_build.stderr
    );

    let (_, res_test) = RustAdapter::build(&opt_test).expect("build test proc macro");
    assert!(
        res_test.success,
        "Standalone proc-macro test build failed: {}",
        res_test.stderr
    );
}

#[test]
fn test_workspace_macro_selection_without_active_consumer() {
    let isolated_cache = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();

    fs::write(
        root.path().join("Cargo.toml"),
        "[workspace]\nmembers = [\"my_macro\", \"unused_app\"]\nresolver = \"2\"\n",
    )
    .unwrap();

    let macro_dir = root.path().join("my_macro");
    write_crate(
        &macro_dir,
        "[package]\nname = \"my_macro\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\nproc-macro = true\n",
        "extern crate proc_macro;\nuse proc_macro::TokenStream;\n#[proc_macro]\npub fn noop(item: TokenStream) -> TokenStream { item }\n",
    );

    let app_dir = root.path().join("unused_app");
    fs::create_dir_all(app_dir.join("src")).unwrap();
    fs::write(
        app_dir.join("Cargo.toml"),
        "[package]\nname = \"unused_app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(app_dir.join("src/main.rs"), "fn main() {}\n").unwrap();

    // Select -p my_macro: macro has no active consumer
    let opt = RustBuildOptions {
        manifest_path: Some(root.path().join("Cargo.toml")),
        cache_dir: Some(isolated_cache.path().to_path_buf()),
        target_dir: Some(root.path().join("target")),
        package: Some("my_macro".to_string()),
        ..Default::default()
    };

    let plan = RustAdapter::explain_plan(&opt).expect("explain plan");
    let macro_unit = plan
        .units
        .iter()
        .find(|u| u.package_name == "my_macro")
        .expect("my_macro unit");
    assert!(
        macro_unit.for_host,
        "selected workspace proc-macro without consumer must be for_host"
    );
    assert!(
        !plan.units.iter().any(|u| u.package_name == "unused_app"),
        "unused_app must be pruned from plan"
    );

    let (_, res) = RustAdapter::build(&opt).expect("build");
    assert!(
        res.success,
        "Workspace -p proc-macro build failed: {}",
        res.stderr
    );
}

#[test]
fn test_proc_macro_resolver_2_decoupling_and_resolver_1_unification() {
    let macro_toml = CargoToml::parse(
        r#"
[package]
name = "pm_crate"
version = "0.1.0"
edition = "2021"

[lib]
proc-macro = true

[dependencies]
shared = { version = "1.0.0", features = ["host_feat"] }
"#,
    )
    .unwrap();

    let app_toml = CargoToml::parse(
        r#"
[package]
name = "app_crate"
version = "0.1.0"
edition = "2021"

[dependencies]
pm_crate = { version = "0.1.0" }
shared = { version = "1.0.0", features = ["target_feat"] }
"#,
    )
    .unwrap();

    let shared_toml = CargoToml::parse(
        r#"
[package]
name = "shared"
version = "1.0.0"
edition = "2021"

[features]
host_feat = []
target_feat = []
"#,
    )
    .unwrap();

    let shared_deps = Vec::new();
    let pm_deps = vec!["shared 1.0.0".to_string()];
    let app_deps = vec!["pm_crate 0.1.0".to_string(), "shared 1.0.0".to_string()];

    let pkgs = vec![
        PackageFeatureContext {
            name: "app_crate",
            version: "0.1.0",
            source: None,
            is_workspace: true,
            is_root: true,
            toml: &app_toml,
            dependencies: &app_deps,
        },
        PackageFeatureContext {
            name: "pm_crate",
            version: "0.1.0",
            source: None,
            is_workspace: true,
            is_root: false,
            toml: &macro_toml,
            dependencies: &pm_deps,
        },
        PackageFeatureContext {
            name: "shared",
            version: "1.0.0",
            source: None,
            is_workspace: false,
            is_root: false,
            toml: &shared_toml,
            dependencies: &shared_deps,
        },
    ];

    let opt = RustBuildOptions::default();

    // Resolver 2: decoupled host and target features
    let res2 = resolve_package_features_with_resolver(&pkgs, &opt, Some("2")).unwrap();
    let shared_target_feats = res2.target.get("shared@1.0.0").cloned().unwrap_or_default();
    let shared_host_feats = res2.host.get("shared@1.0.0").cloned().unwrap_or_default();
    assert!(
        shared_target_feats.contains(&"target_feat".to_string()),
        "target shared must have target_feat"
    );
    assert!(
        !shared_target_feats.contains(&"host_feat".to_string()),
        "Resolver 2: target shared must not have host_feat"
    );
    assert!(
        shared_host_feats.contains(&"host_feat".to_string()),
        "host shared must have host_feat"
    );
    assert!(
        !shared_host_feats.contains(&"target_feat".to_string()),
        "Resolver 2: host shared must not have target_feat"
    );

    // Resolver 1: unified host and target features
    let res1 = resolve_package_features_with_resolver(&pkgs, &opt, Some("1")).unwrap();
    let r1_target = res1.target.get("shared@1.0.0").cloned().unwrap_or_default();
    let r1_host = res1.host.get("shared@1.0.0").cloned().unwrap_or_default();
    assert!(
        r1_target.contains(&"target_feat".to_string())
            && r1_target.contains(&"host_feat".to_string()),
        "Resolver 1: target must unify host and target features"
    );
    assert_eq!(
        r1_target, r1_host,
        "Resolver 1: target and host features must be identical"
    );
}
