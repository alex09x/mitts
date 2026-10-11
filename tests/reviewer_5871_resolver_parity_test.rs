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
    let syn_toml: CargoToml = toml::from_str(
        r#"
        [package]
        name = "syn"
        version = "2.0.0"
        edition = "2021"

        [features]
        default = []
        "proc-macro" = []
        "clone-impls" = []
    "#,
    )
    .unwrap();

    let pm_toml: CargoToml = toml::from_str(
        r#"
        [package]
        name = "my_proc_macro"
        version = "0.1.0"
        edition = "2021"

        [lib]
        proc-macro = true

        [dependencies]
        syn = { version = "2.0.0", features = ["proc-macro"] }
    "#,
    )
    .unwrap();

    let lib_toml: CargoToml = toml::from_str(
        r#"
        [package]
        name = "my_lib"
        version = "0.1.0"
        edition = "2021"

        [dependencies]
        my_proc_macro = { version = "0.1.0" }
        syn = { version = "2.0.0", features = ["clone-impls"] }
    "#,
    )
    .unwrap();

    let syn_deps = vec![];
    let pm_deps = vec!["syn@2.0.0".to_string()];
    let lib_deps = vec!["my_proc_macro@0.1.0".to_string(), "syn@2.0.0".to_string()];

    let packages = setup_test_packages(
        &syn_toml, &pm_toml, &lib_toml, &syn_deps, &pm_deps, &lib_deps,
    );
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
    let syn_toml: CargoToml = toml::from_str(
        r#"
        [package]
        name = "syn"
        version = "2.0.0"
        edition = "2021"

        [features]
        default = []
        "proc-macro" = []
        "clone-impls" = []
    "#,
    )
    .unwrap();

    let pm_toml: CargoToml = toml::from_str(
        r#"
        [package]
        name = "my_proc_macro"
        version = "0.1.0"
        edition = "2021"

        [lib]
        proc-macro = true

        [dependencies]
        syn = { version = "2.0.0", features = ["proc-macro"] }
    "#,
    )
    .unwrap();

    let lib_toml: CargoToml = toml::from_str(
        r#"
        [package]
        name = "my_lib"
        version = "0.1.0"
        edition = "2021"

        [dependencies]
        my_proc_macro = { version = "0.1.0" }
        syn = { version = "2.0.0", features = ["clone-impls"] }
    "#,
    )
    .unwrap();

    let syn_deps = vec![];
    let pm_deps = vec!["syn@2.0.0".to_string()];
    let lib_deps = vec!["my_proc_macro@0.1.0".to_string(), "syn@2.0.0".to_string()];

    let packages = setup_test_packages(
        &syn_toml, &pm_toml, &lib_toml, &syn_deps, &pm_deps, &lib_deps,
    );
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
    let toml: CargoToml = toml::from_str(
        r#"
        [workspace]
        members = ["crate_a", "crate_b"]
    "#,
    )
    .unwrap();
    assert_eq!(toml.effective_resolver(), "1");
}

#[test]
fn test_effective_resolver_root_package_edition_2021_is_2() {
    let toml: CargoToml = toml::from_str(
        r#"
        [package]
        name = "my_pkg"
        version = "0.1.0"
        edition = "2021"
    "#,
    )
    .unwrap();
    assert_eq!(toml.effective_resolver(), "2");
}

#[test]
fn test_effective_resolver_explicit_resolver() {
    let ws_explicit_2: CargoToml = toml::from_str(
        r#"
        [workspace]
        members = ["crate_a"]
        resolver = "2"
    "#,
    )
    .unwrap();
    assert_eq!(ws_explicit_2.effective_resolver(), "2");

    let ws_explicit_1: CargoToml = toml::from_str(
        r#"
        [workspace]
        members = ["crate_a"]
        resolver = "1"
    "#,
    )
    .unwrap();
    assert_eq!(ws_explicit_1.effective_resolver(), "1");

    let pkg_explicit_1: CargoToml = toml::from_str(
        r#"
        [package]
        name = "my_pkg"
        version = "0.1.0"
        edition = "2021"
        resolver = "1"
    "#,
    )
    .unwrap();
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
    )
    .unwrap();

    let shared_dir = root_dir.join("shared_dep");
    std::fs::create_dir_all(shared_dir.join("src")).unwrap();
    std::fs::write(shared_dir.join("src/lib.rs"), "pub fn shared() {}").unwrap();
    std::fs::write(
        shared_dir.join("Cargo.toml"),
        r#"
        [package]
        name = "shared_dep"
        version = "0.1.0"
        edition = "2021"

        [features]
        default = []
        feature_a = []
        feature_b = []
        "#,
    )
    .unwrap();

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

        [dependencies]
        shared_dep = { path = "../shared_dep", features = ["feature_a"] }
        "#,
    )
    .unwrap();

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

        [dependencies]
        shared_dep = { path = "../shared_dep", features = ["feature_b"] }
        "#,
    )
    .unwrap();

    // Verify effective resolver from root
    let root_toml = CargoToml::from_file(&root_dir.join("Cargo.toml")).unwrap();
    let res_from_root =
        CargoToml::effective_resolver_for_manifest(&root_dir.join("Cargo.toml"), &root_toml)
            .unwrap();
    assert_eq!(
        res_from_root, "1",
        "Virtual workspace root without resolver must default to resolver 1"
    );

    // Verify effective resolver when launched from member directory
    let core_toml = CargoToml::from_file(&core_dir.join("Cargo.toml")).unwrap();
    let res_from_member =
        CargoToml::effective_resolver_for_manifest(&core_dir.join("Cargo.toml"), &core_toml)
            .unwrap();
    assert_eq!(
        res_from_member, "1",
        "Member of virtual workspace without resolver must inherit resolver 1"
    );

    // Verify explain_plan succeeds on this layout and unifies features of shared_dep across host/target under resolver 1
    let opt = RustBuildOptions {
        manifest_path: Some(root_dir.join("Cargo.toml")),
        ..Default::default()
    };
    let plan = mitts::adapter::RustAdapter::explain_plan(&opt).unwrap();
    assert!(plan.units.iter().any(|u| u.package_name == "serde_core"));
    assert!(plan.units.iter().any(|u| u.package_name == "serde_derive"));

    let shared_unit = plan
        .units
        .iter()
        .find(|u| u.package_name == "shared_dep")
        .expect("shared_dep unit planned");
    assert!(
        shared_unit.features.contains(&"feature_a".to_string()),
        "feature_a unified under resolver 1"
    );
    assert!(
        shared_unit.features.contains(&"feature_b".to_string()),
        "feature_b unified under resolver 1"
    );
}

#[test]
fn test_find_workspace_root_members_exclude_and_package_workspace() {
    let temp = tempfile::tempdir().unwrap();
    let root_dir = temp.path();

    std::fs::write(
        root_dir.join("Cargo.toml"),
        r#"
        [workspace]
        members = ["crates/*"]
        exclude = ["crates/ignored_crate"]
        resolver = "2"
        "#,
    )
    .unwrap();

    let member_dir = root_dir.join("crates/member_crate");
    std::fs::create_dir_all(member_dir.join("src")).unwrap();
    std::fs::write(member_dir.join("src/lib.rs"), "pub fn m() {}").unwrap();
    std::fs::write(
        member_dir.join("Cargo.toml"),
        r#"
        [package]
        name = "member_crate"
        version = "0.1.0"
        edition = "2021"
        "#,
    )
    .unwrap();

    let excluded_dir = root_dir.join("crates/ignored_crate");
    std::fs::create_dir_all(excluded_dir.join("src")).unwrap();
    std::fs::write(excluded_dir.join("src/lib.rs"), "pub fn ex() {}").unwrap();
    std::fs::write(
        excluded_dir.join("Cargo.toml"),
        r#"
        [package]
        name = "ignored_crate"
        version = "0.1.0"
        edition = "2021"
        "#,
    )
    .unwrap();

    let explicit_ws_dir = root_dir.join("crates/sub/explicit_crate");
    std::fs::create_dir_all(explicit_ws_dir.join("src")).unwrap();
    std::fs::write(explicit_ws_dir.join("src/lib.rs"), "pub fn exp() {}").unwrap();
    std::fs::write(
        explicit_ws_dir.join("Cargo.toml"),
        r#"
        [package]
        name = "explicit_crate"
        version = "0.1.0"
        edition = "2021"
        workspace = "../../.."
        "#,
    )
    .unwrap();

    let member_toml = CargoToml::from_file(&member_dir.join("Cargo.toml")).unwrap();
    let found_member =
        CargoToml::find_workspace_root(&member_dir.join("Cargo.toml"), &member_toml).unwrap();
    assert!(
        found_member.is_some(),
        "member_crate in crates/* must find workspace root"
    );
    assert_eq!(
        found_member.unwrap().0.canonicalize().unwrap(),
        root_dir.join("Cargo.toml").canonicalize().unwrap()
    );

    let excluded_toml = CargoToml::from_file(&excluded_dir.join("Cargo.toml")).unwrap();
    let found_excluded =
        CargoToml::find_workspace_root(&excluded_dir.join("Cargo.toml"), &excluded_toml).unwrap();
    assert!(
        found_excluded.is_none(),
        "ignored_crate in exclude must NOT find workspace root"
    );

    let explicit_toml = CargoToml::from_file(&explicit_ws_dir.join("Cargo.toml")).unwrap();
    let found_explicit =
        CargoToml::find_workspace_root(&explicit_ws_dir.join("Cargo.toml"), &explicit_toml)
            .unwrap();
    assert!(
        found_explicit.is_some(),
        "explicit_crate with package.workspace must find workspace root"
    );
    assert_eq!(
        found_explicit.unwrap().0.canonicalize().unwrap(),
        root_dir.join("Cargo.toml").canonicalize().unwrap()
    );
}

#[test]
fn test_find_workspace_root_fail_closed_and_pattern_parity() {
    let temp = tempfile::tempdir().unwrap();
    let root_dir = temp.path();

    std::fs::write(
        root_dir.join("Cargo.toml"),
        r#"
        [workspace]
        members = ["crates/tokio-*", "crates/core"]
        exclude = ["crates/tokio-ignored"]
        resolver = "2"
        "#,
    )
    .unwrap();

    // 1. Prefix wildcard match: crates/tokio-macros
    let prefix_match_dir = root_dir.join("crates/tokio-macros");
    std::fs::create_dir_all(prefix_match_dir.join("src")).unwrap();
    std::fs::write(prefix_match_dir.join("src/lib.rs"), "pub fn tm() {}").unwrap();
    std::fs::write(
        prefix_match_dir.join("Cargo.toml"),
        r#"
        [package]
        name = "tokio-macros"
        version = "0.1.0"
        edition = "2021"
        "#,
    )
    .unwrap();
    let prefix_toml = CargoToml::from_file(&prefix_match_dir.join("Cargo.toml")).unwrap();
    let found_prefix =
        CargoToml::find_workspace_root(&prefix_match_dir.join("Cargo.toml"), &prefix_toml).unwrap();
    assert!(
        found_prefix.is_some(),
        "crates/tokio-macros must match prefix wildcard crates/tokio-*"
    );

    // 2. Excluded under prefix: crates/tokio-ignored
    let ignored_dir = root_dir.join("crates/tokio-ignored");
    std::fs::create_dir_all(ignored_dir.join("src")).unwrap();
    std::fs::write(ignored_dir.join("src/lib.rs"), "pub fn ti() {}").unwrap();
    std::fs::write(
        ignored_dir.join("Cargo.toml"),
        r#"
        [package]
        name = "tokio-ignored"
        version = "0.1.0"
        edition = "2021"
        "#,
    )
    .unwrap();
    let ignored_toml = CargoToml::from_file(&ignored_dir.join("Cargo.toml")).unwrap();
    let found_ignored =
        CargoToml::find_workspace_root(&ignored_dir.join("Cargo.toml"), &ignored_toml).unwrap();
    assert!(
        found_ignored.is_none(),
        "crates/tokio-ignored is excluded and must not find workspace root"
    );

    // 3. Unlisted package in workspace tree: should fail-closed with Err
    let unlisted_dir = root_dir.join("crates/unlisted");
    std::fs::create_dir_all(unlisted_dir.join("src")).unwrap();
    std::fs::write(unlisted_dir.join("src/lib.rs"), "pub fn ul() {}").unwrap();
    std::fs::write(
        unlisted_dir.join("Cargo.toml"),
        r#"
        [package]
        name = "unlisted"
        version = "0.1.0"
        edition = "2021"
        "#,
    )
    .unwrap();
    let unlisted_toml = CargoToml::from_file(&unlisted_dir.join("Cargo.toml")).unwrap();
    let err_unlisted =
        CargoToml::find_workspace_root(&unlisted_dir.join("Cargo.toml"), &unlisted_toml);
    assert!(
        err_unlisted.is_err(),
        "Unlisted crate inside workspace tree must fail-closed"
    );
    let err_msg = err_unlisted.unwrap_err().to_string();
    assert!(err_msg.contains("is not listed in `workspace.members` or `workspace.exclude`"));

    // 4. Invalid package.workspace pointing to nonexistent path: should fail-closed with Err
    let broken_ws_dir = root_dir.join("broken_ws");
    std::fs::create_dir_all(broken_ws_dir.join("src")).unwrap();
    std::fs::write(broken_ws_dir.join("src/lib.rs"), "pub fn bw() {}").unwrap();
    std::fs::write(
        broken_ws_dir.join("Cargo.toml"),
        r#"
        [package]
        name = "broken_ws"
        version = "0.1.0"
        edition = "2021"
        workspace = "../nonexistent_workspace"
        "#,
    )
    .unwrap();
    let broken_toml = CargoToml::from_file(&broken_ws_dir.join("Cargo.toml")).unwrap();
    let err_broken =
        CargoToml::find_workspace_root(&broken_ws_dir.join("Cargo.toml"), &broken_toml);
    assert!(
        err_broken.is_err(),
        "package.workspace pointing to nonexistent path must fail-closed"
    );
    assert!(err_broken
        .unwrap_err()
        .to_string()
        .contains("does not exist or is not a file"));

    // 5. Invalid package.workspace pointing to a Cargo.toml without [workspace]: should fail-closed with Err
    let not_ws_dir = root_dir.join("not_ws");
    std::fs::create_dir_all(not_ws_dir.join("src")).unwrap();
    std::fs::write(not_ws_dir.join("src/lib.rs"), "pub fn nw() {}").unwrap();
    std::fs::write(
        not_ws_dir.join("Cargo.toml"),
        r#"
        [package]
        name = "not_ws"
        version = "0.1.0"
        edition = "2021"
        workspace = "../crates/tokio-macros"
        "#,
    )
    .unwrap();
    let not_ws_toml = CargoToml::from_file(&not_ws_dir.join("Cargo.toml")).unwrap();
    let err_not_ws = CargoToml::find_workspace_root(&not_ws_dir.join("Cargo.toml"), &not_ws_toml);
    assert!(
        err_not_ws.is_err(),
        "package.workspace pointing to non-workspace Cargo.toml must fail-closed"
    );
    assert!(err_not_ws
        .unwrap_err()
        .to_string()
        .contains("does not contain a [workspace] section"));
}

#[test]
fn test_workspace_pattern_unsupported_glob_syntax() {
    use mitts::adapter::rust::manifest_workspace::validate_workspace_pattern;

    assert!(validate_workspace_pattern("crates/*").is_ok());
    assert!(validate_workspace_pattern("crates/**").is_ok());
    assert!(validate_workspace_pattern("crates/tokio-*").is_ok());

    let err_inter = validate_workspace_pattern("crates/*/sub");
    assert!(err_inter.is_err());
    assert!(err_inter
        .unwrap_err()
        .to_string()
        .contains("wildcards in intermediate directories are not supported"));

    let err_q = validate_workspace_pattern("crates/?/foo");
    assert!(err_q.is_err());
    assert!(err_q
        .unwrap_err()
        .to_string()
        .contains("glob character classes and `?` wildcards are not supported"));

    let err_bracket = validate_workspace_pattern("crates/[a-z]");
    assert!(err_bracket.is_err());
    assert!(err_bracket
        .unwrap_err()
        .to_string()
        .contains("glob character classes and `?` wildcards are not supported"));

    let err_multi = validate_workspace_pattern("crates/*foo*");
    assert!(err_multi.is_err());
    assert!(err_multi
        .unwrap_err()
        .to_string()
        .contains("multiple wildcards in a component are not supported"));
}
