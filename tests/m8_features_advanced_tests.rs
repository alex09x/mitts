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
fn test_m8_features_cross_crate_and_weak_dependencies() {
    let root_toml_str = r#"
[package]
name = "root-app"
version = "0.1.0"

[features]
default = ["std"]
std = []
gui = ["dep_a/gui_mode", "dep_b?/fast_mode", "dep_inactive?/skipped"]

[dependencies]
dep_a = "1.0"
dep_b = "1.0"
"#;
    let root_toml = CargoToml::parse(root_toml_str).unwrap();

    let dep_a_toml_str = r#"
[package]
name = "dep_a"
version = "1.0.0"

[features]
default = []
gui_mode = ["backend"]
backend = []
"#;
    let dep_a_toml = CargoToml::parse(dep_a_toml_str).unwrap();

    let dep_b_toml_str = r#"
[package]
name = "dep_b"
version = "1.0.0"

[features]
default = []
fast_mode = []
"#;
    let dep_b_toml = CargoToml::parse(dep_b_toml_str).unwrap();

    let dep_inactive_toml_str = r#"
[package]
name = "dep_inactive"
version = "1.0.0"

[features]
default = []
skipped = []
"#;
    let dep_inactive_toml = CargoToml::parse(dep_inactive_toml_str).unwrap();

    let root_deps = vec!["dep_a 1.0.0".to_string(), "dep_b 1.0.0".to_string()];
    let pkgs = vec![
        PackageFeatureContext {
            name: "root-app",
            version: "0.1.0",
            is_workspace: true,
            is_root: true,
            toml: &root_toml,
            dependencies: &root_deps,
        },
        PackageFeatureContext {
            name: "dep_a",
            version: "1.0.0",
            is_workspace: false,
            is_root: false,
            toml: &dep_a_toml,
            dependencies: &[],
        },
        PackageFeatureContext {
            name: "dep_b",
            version: "1.0.0",
            is_workspace: false,
            is_root: false,
            toml: &dep_b_toml,
            dependencies: &[],
        },
        PackageFeatureContext {
            name: "dep_inactive",
            version: "1.0.0",
            is_workspace: false,
            is_root: false,
            toml: &dep_inactive_toml,
            dependencies: &[],
        },
    ];

    let opt = RustBuildOptions {
        features: vec!["gui".to_string()],
        ..Default::default()
    };

    let resolved = resolve_package_features(&pkgs, &opt);

    // root-app has default, std, gui
    let root_feats = resolved.get("root-app@0.1.0").unwrap();
    assert!(root_feats.contains(&"default".to_string()));
    assert!(root_feats.contains(&"std".to_string()));
    assert!(root_feats.contains(&"gui".to_string()));

    // dep_a received gui_mode, which expanded into backend
    let dep_a_feats = resolved.get("dep_a@1.0.0").unwrap();
    assert!(dep_a_feats.contains(&"gui_mode".to_string()));
    assert!(dep_a_feats.contains(&"backend".to_string()));

    // dep_b is active, so weak dep_b?/fast_mode activated fast_mode
    let dep_b_feats = resolved.get("dep_b@1.0.0").unwrap();
    assert!(dep_b_feats.contains(&"fast_mode".to_string()));

    // dep_inactive is NOT in dependencies, so dep_inactive?/skipped was NOT activated
    let dep_inact_feats = resolved.get("dep_inactive@1.0.0").unwrap();
    assert!(!dep_inact_feats.contains(&"skipped".to_string()));
}

#[test]
fn test_m8_features_target_dependencies_and_aliases() {
    let toml_str = r#"
[package]
name = "alias-app"
version = "0.2.0"

[dependencies.renamed_lib]
package = "actual_lib"
version = "2.0"
features = ["feature_x"]
default-features = false

[target.'cfg(unix)'.dependencies]
unix_dep = { version = "0.5", default-features = false }
"#;
    let toml = CargoToml::parse(toml_str).unwrap();

    let actual_lib_toml_str = r#"
[package]
name = "actual_lib"
version = "2.0.0"

[features]
default = ["def"]
def = []
feature_x = []
"#;
    let actual_lib_toml = CargoToml::parse(actual_lib_toml_str).unwrap();

    let unix_dep_toml_str = r#"
[package]
name = "unix_dep"
version = "0.5.0"

[features]
default = ["unix_default"]
unix_default = []
"#;
    let unix_dep_toml = CargoToml::parse(unix_dep_toml_str).unwrap();

    let alias_deps = vec!["actual_lib 2.0.0".to_string(), "unix_dep 0.5.0".to_string()];
    let pkgs = vec![
        PackageFeatureContext {
            name: "alias-app",
            version: "0.2.0",
            is_workspace: true,
            is_root: true,
            toml: &toml,
            dependencies: &alias_deps,
        },
        PackageFeatureContext {
            name: "actual_lib",
            version: "2.0.0",
            is_workspace: false,
            is_root: false,
            toml: &actual_lib_toml,
            dependencies: &[],
        },
        PackageFeatureContext {
            name: "unix_dep",
            version: "0.5.0",
            is_workspace: false,
            is_root: false,
            toml: &unix_dep_toml,
            dependencies: &[],
        },
    ];

    let opt = RustBuildOptions::default();
    let resolved = resolve_package_features(&pkgs, &opt);

    // actual_lib has default-features = false, but feature_x is requested
    let actual_feats = resolved.get("actual_lib@2.0.0").unwrap();
    assert!(actual_feats.contains(&"feature_x".to_string()));
    assert!(!actual_feats.contains(&"default".to_string()));

    // unix_dep has default-features = false, so default is not activated
    let unix_feats = resolved.get("unix_dep@0.5.0").unwrap();
    assert!(!unix_feats.contains(&"default".to_string()));
}
