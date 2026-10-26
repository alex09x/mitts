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
use mitts::adapter::rust::manifest_inherit::{
    extract_check_cfg_from_lints, extract_lint_flags_from_lints, resolve_inherited_workspace_fields,
};
use std::fs;
use tempfile::tempdir;

#[test]
fn test_workspace_lints_inheritance_and_check_cfg() {
    let tmp = tempdir().unwrap();
    let ws_root = tmp.path();

    let root_toml = r#"
[workspace]
members = ["crates/member_a"]

[workspace.lints.rust]
unexpected_cfgs = { level = "warn", check-cfg = ['cfg(tokio_unstable)', 'cfg(loom)'] }
missing_docs = "warn"
unsafe_code = "deny"

[workspace.lints.clippy]
dbg_macro = "deny"
"#;
    fs::write(ws_root.join("Cargo.toml"), root_toml).unwrap();

    let member_dir = ws_root.join("crates").join("member_a");
    fs::create_dir_all(&member_dir).unwrap();
    let member_toml_content = r#"
[package]
name = "member_a"
version = "0.1.0"
edition = "2021"

[lints]
workspace = true
"#;
    let member_manifest_path = member_dir.join("Cargo.toml");
    fs::write(&member_manifest_path, member_toml_content).unwrap();

    let mut member_toml = CargoToml::from_file(&member_manifest_path).unwrap();
    resolve_inherited_workspace_fields(&mut member_toml, &member_manifest_path);

    assert!(
        member_toml.lints.is_some(),
        "lints should be inherited from workspace"
    );
    let lints_val = member_toml.lints.as_ref().unwrap();

    let check_cfgs = extract_check_cfg_from_lints(lints_val);
    assert!(check_cfgs.contains(&"cfg(tokio_unstable)".to_string()));
    assert!(check_cfgs.contains(&"cfg(loom)".to_string()));

    let lint_flags = extract_lint_flags_from_lints(lints_val);
    assert!(lint_flags.contains(&"-Wmissing_docs".to_string()));
    assert!(lint_flags.contains(&"-Dunsafe_code".to_string()));
    assert!(lint_flags.contains(&"-Dclippy::dbg_macro".to_string()));
}

#[test]
fn test_target_specific_optional_dependencies_in_check_cfg() {
    let tmp = tempdir().unwrap();
    let manifest_path = tmp.path().join("Cargo.toml");

    let toml_content = r#"
[package]
name = "target_dep_crate"
version = "1.0.0"
edition = "2021"

[features]
default = []
custom_feat = []

[dependencies]
direct_dep = { version = "1.0", optional = true }

[target.'cfg(unix)'.dependencies]
mio_uds = { version = "0.6", optional = true }
libc = "0.2"

[target.'cfg(windows)'.dependencies]
winapi = { version = "0.3", optional = true }
"#;
    fs::write(&manifest_path, toml_content).unwrap();

    let toml = CargoToml::from_file(&manifest_path).unwrap();

    let mut all_features: Vec<String> = toml.features.keys().cloned().collect();
    for (dep_name, dep_val) in &toml.dependencies {
        if let mitts::adapter::rust::manifest::TomlDependency::Detailed(ref d) = dep_val {
            if d.optional == Some(true) {
                all_features.push(dep_name.clone());
            }
        }
    }
    for target_spec in toml.target.values() {
        for (dep_name, dep_val) in &target_spec.dependencies {
            if let mitts::adapter::rust::manifest::TomlDependency::Detailed(ref d) = dep_val {
                if d.optional == Some(true) {
                    all_features.push(dep_name.clone());
                }
            }
        }
    }
    all_features.sort();
    all_features.dedup();

    assert!(all_features.contains(&"custom_feat".to_string()));
    assert!(all_features.contains(&"direct_dep".to_string()));
    assert!(
        all_features.contains(&"mio_uds".to_string()),
        "Target-specific optional dependency mio_uds should be included in feature values"
    );
    assert!(
        all_features.contains(&"winapi".to_string()),
        "Target-specific optional dependency winapi should be included in feature values"
    );
    assert!(
        !all_features.contains(&"libc".to_string()),
        "Non-optional dependency libc should not be in feature values"
    );
}

#[test]
fn test_target_specific_dev_dependencies_in_test_mode() {
    let manifest_str = r#"
[package]
name = "my-test-crate"
version = "0.1.0"
edition = "2021"

[dependencies]
serde = "1.0"

[target.'cfg(all())'.dev-dependencies]
tempfile = { version = "3.8", features = ["nitro"] }
"#;

    let dir = tempfile::tempdir().unwrap();
    let manifest_path = dir.path().join("Cargo.toml");
    std::fs::write(&manifest_path, manifest_str).unwrap();

    let toml = CargoToml::from_file(&manifest_path).unwrap();
    let target_spec = toml.target.get("cfg(all())").expect("cfg(all()) found");
    assert!(target_spec.dev_dependencies.contains_key("tempfile"));

    let is_dep_in_test = mitts::adapter::rust::package_selection::is_declared_dependency(
        &toml,
        "tempfile",
        true,
        None,
        std::path::Path::new("."),
    )
    .unwrap();
    assert!(
        is_dep_in_test,
        "target dev-dependency must be recognized in test mode"
    );

    let is_dep_in_build = mitts::adapter::rust::package_selection::is_declared_dependency(
        &toml,
        "tempfile",
        false,
        None,
        std::path::Path::new("."),
    )
    .unwrap();
    assert!(
        !is_dep_in_build,
        "target dev-dependency must not be recognized in non-test mode"
    );
}

#[test]
fn test_m8_target_has_atomic_filtering() {
    use mitts::adapter::rust::target_filter::is_target_cfg_incompatible_with_host;

    // On modern host (x86_64 / aarch64), target_has_atomic = "ptr" is true.
    // Therefore cfg(not(target_has_atomic = "ptr")) must be incompatible (evaluated to false).
    assert!(
        is_target_cfg_incompatible_with_host("cfg(not(target_has_atomic = \"ptr\"))"),
        "not(target_has_atomic = ptr) must be marked incompatible on host"
    );
    assert!(
        !is_target_cfg_incompatible_with_host("cfg(target_has_atomic = \"ptr\")"),
        "target_has_atomic = ptr must be marked compatible on host"
    );
    assert!(
        !is_target_cfg_incompatible_with_host("cfg(target_has_atomic = \"64\")"),
        "target_has_atomic = 64 must be marked compatible on host"
    );
}

#[test]
fn test_m8_build_disabled_ignores_build_dependencies() {
    use mitts::adapter::rust::manifest::CargoToml;
    use mitts::adapter::rust::package_selection::is_declared_dependency;

    let manifest_str = r#"
        [package]
        name = "test-pkg"
        version = "0.1.0"
        edition = "2021"
        build = false

        [build-dependencies]
        rustc_version = "0.4"
    "#;
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = dir.path().join("Cargo.toml");
    std::fs::write(&manifest_path, manifest_str).unwrap();

    let toml = CargoToml::from_file(&manifest_path).unwrap();
    assert!(
        !is_declared_dependency(
            &toml,
            "rustc_version",
            false,
            None,
            std::path::Path::new(".")
        )
        .unwrap(),
        "build_dependencies must be ignored when build = false"
    );
}

#[test]
fn test_m8_host_and_target_cfg_and_profile_debuginfo() {
    use mitts::adapter::rust::key_flags::resolve_unit_debuginfo;
    use mitts::adapter::rust::target_filter::is_target_cfg_incompatible_with_target;

    let res_target = is_target_cfg_incompatible_with_target(
        "cfg(target_os = \"nonexistent_os\")",
        false,
        None,
        std::path::Path::new("."),
    );
    assert!(res_target.unwrap());

    let res_host = is_target_cfg_incompatible_with_target(
        "cfg(target_os = \"nonexistent_os\")",
        true,
        None,
        std::path::Path::new("."),
    );
    assert!(res_host.unwrap());

    let manifest_str = r#"
        [package]
        name = "test-profile-pkg"
        version = "0.1.0"
        edition = "2021"

        [profile.dev]
        debug = 1

        [profile.dev.build-override]
        debug = 0
    "#;
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = dir.path().join("Cargo.toml");
    std::fs::write(&manifest_path, manifest_str).unwrap();

    let dbg_target = resolve_unit_debuginfo(
        &manifest_path,
        "test-profile-pkg",
        true,
        false,
        false,
        Some(dir.path()),
    )
    .unwrap();
    assert_eq!(
        dbg_target, 1,
        "target dev debuginfo should read [profile.dev].debug"
    );

    let dbg_host = resolve_unit_debuginfo(
        &manifest_path,
        "test-profile-pkg",
        true,
        true,
        false,
        Some(dir.path()),
    )
    .unwrap();
    assert_eq!(
        dbg_host, 0,
        "host dev debuginfo should read [profile.dev.build-override].debug"
    );
}

/// `[lints.cargo]` belongs to Cargo's own lint set; rustc rejects `cargo::` lints
/// (E0602), so they must never reach the compiler command line (actix-tls 3.6.1).
#[test]
fn cargo_lints_are_not_forwarded_to_rustc() {
    let lints: toml::Value = toml::from_str(
        "[cargo]\nunused_dependencies = \"allow\"\n[rust]\nmissing_docs = \"warn\"\n",
    )
    .unwrap();
    assert_eq!(
        extract_lint_flags_from_lints(&lints),
        vec!["-Wmissing_docs"]
    );
}
