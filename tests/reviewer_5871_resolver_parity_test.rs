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
use mitts::adapter::rust::types::RustBuildOptions;

fn setup_test_packages<'a>(
    syn_toml: &'a CargoToml,
    pm_toml: &'a CargoToml,
    lib_toml: &'a CargoToml,
    syn_deps: &'a [String],
    pm_deps: &'a [String],
    lib_deps: &'a [String],
) -> Vec<PackageFeatureContext<'a>> {
    vec![
        PackageFeatureContext {
            name: "syn",
            version: "2.0.0",
            is_workspace: false,
            is_root: false,
            toml: syn_toml,
            dependencies: syn_deps,
        },
        PackageFeatureContext {
            name: "my_proc_macro",
            version: "0.1.0",
            is_workspace: false,
            is_root: false,
            toml: pm_toml,
            dependencies: pm_deps,
        },
        PackageFeatureContext {
            name: "my_lib",
            version: "0.1.0",
            is_workspace: true,
            is_root: true,
            toml: lib_toml,
            dependencies: lib_deps,
        },
    ]
}

#[test]
fn test_resolver_2_decouples_host_and_target_features() {
    let syn_toml: CargoToml = toml::from_str(r#"
        [package]
        name = "syn"
        version = "2.0.0"
        edition = "2021"

        [features]
        default = []
        "proc-macro" = []
        "clone-impls" = []
    "#).unwrap();

    let pm_toml: CargoToml = toml::from_str(r#"
        [package]
        name = "my_proc_macro"
        version = "0.1.0"
        edition = "2021"

        [lib]
        proc-macro = true

        [dependencies]
        syn = { version = "2.0.0", features = ["proc-macro"] }
    "#).unwrap();

    let lib_toml: CargoToml = toml::from_str(r#"
        [package]
        name = "my_lib"
        version = "0.1.0"
        edition = "2021"

        [dependencies]
        my_proc_macro = { version = "0.1.0" }
        syn = { version = "2.0.0", features = ["clone-impls"] }
    "#).unwrap();

    let syn_deps = vec![];
    let pm_deps = vec!["syn@2.0.0".to_string()];
    let lib_deps = vec!["my_proc_macro@0.1.0".to_string(), "syn@2.0.0".to_string()];

    let packages = setup_test_packages(&syn_toml, &pm_toml, &lib_toml, &syn_deps, &pm_deps, &lib_deps);
    let opt = RustBuildOptions::default();

    let res = resolve_package_features_with_resolver(&packages, &opt, Some("2")).unwrap();
    let syn_target_feats = res.target.get("syn@2.0.0").cloned().unwrap_or_default();
    let syn_host_feats = res.host.get("syn@2.0.0").cloned().unwrap_or_default();

    assert!(
        syn_target_feats.contains(&"clone-impls".to_string()),
        "Target syn must receive clone-impls from my_lib"
    );
    assert!(
        !syn_target_feats.contains(&"proc-macro".to_string()),
        "Resolver 2: Target syn must NOT receive proc-macro from host proc_macro dep"
    );
    assert!(
        syn_host_feats.contains(&"proc-macro".to_string()),
        "Host syn must receive proc-macro from my_proc_macro"
    );
    assert!(
        !syn_host_feats.contains(&"clone-impls".to_string()),
        "Resolver 2: Host syn must NOT receive clone-impls from target my_lib"
    );
}

#[test]
fn test_resolver_1_unifies_host_and_target_features() {
    let syn_toml: CargoToml = toml::from_str(r#"
        [package]
        name = "syn"
        version = "2.0.0"
        edition = "2021"

        [features]
        default = []
        "proc-macro" = []
        "clone-impls" = []
    "#).unwrap();

    let pm_toml: CargoToml = toml::from_str(r#"
        [package]
        name = "my_proc_macro"
        version = "0.1.0"
        edition = "2021"

        [lib]
        proc-macro = true

        [dependencies]
        syn = { version = "2.0.0", features = ["proc-macro"] }
    "#).unwrap();

    let lib_toml: CargoToml = toml::from_str(r#"
        [package]
        name = "my_lib"
        version = "0.1.0"
        edition = "2021"

        [dependencies]
        my_proc_macro = { version = "0.1.0" }
        syn = { version = "2.0.0", features = ["clone-impls"] }
    "#).unwrap();

    let syn_deps = vec![];
    let pm_deps = vec!["syn@2.0.0".to_string()];
    let lib_deps = vec!["my_proc_macro@0.1.0".to_string(), "syn@2.0.0".to_string()];

    let packages = setup_test_packages(&syn_toml, &pm_toml, &lib_toml, &syn_deps, &pm_deps, &lib_deps);
    let opt = RustBuildOptions::default();

    let res = resolve_package_features_with_resolver(&packages, &opt, Some("1")).unwrap();
    let syn_target_feats = res.target.get("syn@2.0.0").cloned().unwrap_or_default();
    let syn_host_feats = res.host.get("syn@2.0.0").cloned().unwrap_or_default();

    assert!(
        syn_target_feats.contains(&"proc-macro".to_string())
            && syn_target_feats.contains(&"clone-impls".to_string()),
        "Resolver 1: Target syn must be unified with host features"
    );
    assert_eq!(
        syn_target_feats, syn_host_feats,
        "Resolver 1: Target and host features must be completely unified"
    );
}

#[test]
fn test_effective_resolver_virtual_workspace_without_resolver_is_1() {
    let toml: CargoToml = toml::from_str(r#"
        [workspace]
        members = ["crate_a", "crate_b"]
    "#).unwrap();
    assert_eq!(toml.effective_resolver(), "1");
}

#[test]
fn test_effective_resolver_root_package_edition_2021_is_2() {
    let toml: CargoToml = toml::from_str(r#"
        [package]
        name = "my_pkg"
        version = "0.1.0"
        edition = "2021"
    "#).unwrap();
    assert_eq!(toml.effective_resolver(), "2");
}

#[test]
fn test_effective_resolver_explicit_resolver() {
    let ws_explicit_2: CargoToml = toml::from_str(r#"
        [workspace]
        members = ["crate_a"]
        resolver = "2"
    "#).unwrap();
    assert_eq!(ws_explicit_2.effective_resolver(), "2");

    let ws_explicit_1: CargoToml = toml::from_str(r#"
        [workspace]
        members = ["crate_a"]
        resolver = "1"
    "#).unwrap();
    assert_eq!(ws_explicit_1.effective_resolver(), "1");

    let pkg_explicit_1: CargoToml = toml::from_str(r#"
        [package]
        name = "my_pkg"
        version = "0.1.0"
        edition = "2021"
        resolver = "1"
    "#).unwrap();
    assert_eq!(pkg_explicit_1.effective_resolver(), "1");
}

#[test]
fn test_explain_plan_serde_like_layout() {
    let temp = tempfile::tempdir().unwrap();
    let root_dir = temp.path();

    // Virtual workspace root
    std::fs::write(
        root_dir.join("Cargo.toml"),
        r#"
        [workspace]
        members = ["serde_core", "serde_derive"]
        "#,
    ).unwrap();

    let core_dir = root_dir.join("serde_core");
    std::fs::create_dir_all(core_dir.join("src")).unwrap();
    std::fs::write(core_dir.join("src/lib.rs"), "pub fn run() {}").unwrap();
    std::fs::write(
        core_dir.join("Cargo.toml"),
        r#"
        [package]
        name = "serde_core"
        version = "1.0.0"
        edition = "2021"
        "#,
    ).unwrap();

    let derive_dir = root_dir.join("serde_derive");
    std::fs::create_dir_all(derive_dir.join("src")).unwrap();
    std::fs::write(derive_dir.join("src/lib.rs"), "pub fn derive() {}").unwrap();
    std::fs::write(
        derive_dir.join("Cargo.toml"),
        r#"
        [package]
        name = "serde_derive"
        version = "1.0.0"
        edition = "2021"

        [lib]
        proc-macro = true
        "#,
    ).unwrap();

    // Verify effective resolver from root
    let root_toml = CargoToml::from_file(&root_dir.join("Cargo.toml")).unwrap();
    let res_from_root = CargoToml::effective_resolver_for_manifest(&root_dir.join("Cargo.toml"), &root_toml);
    assert_eq!(res_from_root, "1", "Virtual workspace root without resolver must default to resolver 1");

    // Verify effective resolver when launched from member directory
    let core_toml = CargoToml::from_file(&core_dir.join("Cargo.toml")).unwrap();
    let res_from_member = CargoToml::effective_resolver_for_manifest(&core_dir.join("Cargo.toml"), &core_toml);
    assert_eq!(res_from_member, "1", "Member of virtual workspace without resolver must inherit resolver 1");

    // Verify explain_plan succeeds on this layout
    let opt = RustBuildOptions {
        manifest_path: Some(root_dir.join("Cargo.toml")),
        ..Default::default()
    };
    let plan = mitts::adapter::RustAdapter::explain_plan(&opt).unwrap();
    assert_eq!(plan.units.len(), 2, "Both serde_core and serde_derive units planned");
}
