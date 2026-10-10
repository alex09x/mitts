/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::rust::features::{resolve_package_features, PackageFeatureContext};
use mitts::adapter::rust::manifest::CargoToml;
use mitts::adapter::rust::types::RustBuildOptions;

#[test]
fn test_resolver_1_and_native_unifies_host_and_target_features() {
    let syn_toml_str = r#"
        [package]
        name = "syn"
        version = "2.0.0"
        edition = "2021"

        [features]
        default = ["proc-macro"]
        "proc-macro" = []
        "clone-impls" = []
        "parsing" = []
    "#;
    let syn_toml: CargoToml = toml::from_str(syn_toml_str).unwrap();

    let pm_toml_str = r#"
        [package]
        name = "my_proc_macro"
        version = "0.1.0"
        edition = "2021"

        [lib]
        proc-macro = true

        [dependencies]
        syn = { version = "2.0.0", features = ["proc-macro"] }
    "#;
    let pm_toml: CargoToml = toml::from_str(pm_toml_str).unwrap();

    let lib_toml_str = r#"
        [package]
        name = "my_lib"
        version = "0.1.0"
        edition = "2021"

        [dependencies]
        syn = { version = "2.0.0", features = ["clone-impls"], default-features = false }
    "#;
    let lib_toml: CargoToml = toml::from_str(lib_toml_str).unwrap();

    let syn_deps = vec![];
    let pm_deps = vec!["syn@2.0.0".to_string()];
    let lib_deps = vec!["syn@2.0.0".to_string()];

    let syn_ctx = PackageFeatureContext {
        name: "syn",
        version: "2.0.0",
        is_workspace: false,
        is_root: false,
        toml: &syn_toml,
        dependencies: &syn_deps,
    };
    let pm_ctx = PackageFeatureContext {
        name: "my_proc_macro",
        version: "0.1.0",
        is_workspace: true,
        is_root: true,
        toml: &pm_toml,
        dependencies: &pm_deps,
    };
    let lib_ctx = PackageFeatureContext {
        name: "my_lib",
        version: "0.1.0",
        is_workspace: true,
        is_root: true,
        toml: &lib_toml,
        dependencies: &lib_deps,
    };

    let packages = vec![syn_ctx, pm_ctx, lib_ctx];
    let opt = RustBuildOptions::default();

    let res = resolve_package_features(&packages, &opt).unwrap();
    let syn_target_feats = res.target.get("syn@2.0.0").cloned().unwrap_or_default();
    let syn_host_feats = res.host.get("syn@2.0.0").cloned().unwrap_or_default();

    assert!(
        syn_target_feats.contains(&"proc-macro".to_string()),
        "Native build must unify features so target syn gets proc-macro from host dependency"
    );
    assert_eq!(
        syn_target_feats, syn_host_feats,
        "Target and host features for syn must be unified on native build"
    );
}
