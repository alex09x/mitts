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
use mitts::adapter::rust::test_selection::{select_test_targets, validate_test_selection};
use mitts::adapter::rust::types::{BuildMode, RustBuildOptions};

#[test]
fn test_all_features_scoped_to_package_when_package_specified() {
    let pkg_a_toml: CargoToml = toml::from_str(
        r#"
        [package]
        name = "pkg_a"
        version = "0.1.0"
        edition = "2021"

        [features]
        default = []
        feat_a = []
        "#,
    )
    .unwrap();

    let pkg_b_toml: CargoToml = toml::from_str(
        r#"
        [package]
        name = "pkg_b"
        version = "0.1.0"
        edition = "2021"

        [features]
        default = []
        feat_b = []
        "#,
    )
    .unwrap();

    let a_deps = vec![];
    let b_deps = vec![];

    // Case 1: -p pkg_a --all-features -> pkg_a gets all features, pkg_b does NOT
    let packages_scoped = vec![
        PackageFeatureContext {
            name: "pkg_a",
            version: "0.1.0",
            source: None,
            is_workspace: true,
            is_root: true,
            toml: &pkg_a_toml,
            dependencies: &a_deps,
        },
        PackageFeatureContext {
            name: "pkg_b",
            version: "0.1.0",
            source: None,
            is_workspace: true,
            is_root: false,
            toml: &pkg_b_toml,
            dependencies: &b_deps,
        },
    ];

    let opt_scoped = RustBuildOptions {
        package: Some("pkg_a".to_string()),
        all_features: true,
        ..Default::default()
    };

    let resolved_scoped =
        resolve_package_features_with_resolver(&packages_scoped, &opt_scoped, Some("2")).unwrap();
    let feats_a = resolved_scoped
        .target
        .get("pkg_a@0.1.0")
        .cloned()
        .unwrap_or_default();
    let feats_b = resolved_scoped
        .target
        .get("pkg_b@0.1.0")
        .cloned()
        .unwrap_or_default();
    assert!(
        feats_a.contains(&"feat_a".to_string()),
        "pkg_a must have feat_a"
    );
    assert!(
        !feats_b.contains(&"feat_b".to_string()),
        "pkg_b must NOT have feat_b when scoped to pkg_a"
    );

    // Case 2: --workspace --all-features (package is None) -> both get all features
    let packages_all_ws = vec![
        PackageFeatureContext {
            name: "pkg_a",
            version: "0.1.0",
            source: None,
            is_workspace: true,
            is_root: true,
            toml: &pkg_a_toml,
            dependencies: &a_deps,
        },
        PackageFeatureContext {
            name: "pkg_b",
            version: "0.1.0",
            source: None,
            is_workspace: true,
            is_root: true,
            toml: &pkg_b_toml,
            dependencies: &b_deps,
        },
    ];

    let opt_all = RustBuildOptions {
        package: None,
        all_features: true,
        ..Default::default()
    };

    let resolved_all =
        resolve_package_features_with_resolver(&packages_all_ws, &opt_all, Some("2")).unwrap();
    let all_a = resolved_all
        .target
        .get("pkg_a@0.1.0")
        .cloned()
        .unwrap_or_default();
    let all_b = resolved_all
        .target
        .get("pkg_b@0.1.0")
        .cloned()
        .unwrap_or_default();
    assert!(all_a.contains(&"feat_a".to_string()));
    assert!(all_b.contains(&"feat_b".to_string()));
}

#[test]
fn test_dev_dependencies_scoping_resolver_1_and_2() {
    let dep_toml: CargoToml = toml::from_str(
        r#"
        [package]
        name = "shared_dep"
        version = "0.1.0"
        edition = "2021"

        [features]
        default = []
        from_a = []
        from_b = []
        "#,
    )
    .unwrap();

    let a_toml: CargoToml = toml::from_str(
        r#"
        [package]
        name = "pkg_a"
        version = "0.1.0"
        edition = "2021"

        [dev-dependencies]
        shared_dep = { version = "0.1.0", features = ["from_a"] }
        "#,
    )
    .unwrap();

    let b_toml: CargoToml = toml::from_str(
        r#"
        [package]
        name = "pkg_b"
        version = "0.1.0"
        edition = "2021"

        [dev-dependencies]
        shared_dep = { version = "0.1.0", features = ["from_b"] }
        "#,
    )
    .unwrap();

    let a_deps = vec!["shared_dep 0.1.0".to_string()];
    let b_deps = vec!["shared_dep 0.1.0".to_string()];
    let dep_deps = vec![];

    for resolver in ["1", "2"] {
        // Case: Test mode, pkg_a is root, pkg_b is non-root
        let packages = vec![
            PackageFeatureContext {
                name: "shared_dep",
                version: "0.1.0",
                source: None,
                is_workspace: false,
                is_root: false,
                toml: &dep_toml,
                dependencies: &dep_deps,
            },
            PackageFeatureContext {
                name: "pkg_a",
                version: "0.1.0",
                source: None,
                is_workspace: true,
                is_root: true,
                toml: &a_toml,
                dependencies: &a_deps,
            },
            PackageFeatureContext {
                name: "pkg_b",
                version: "0.1.0",
                source: None,
                is_workspace: true,
                is_root: false,
                toml: &b_toml,
                dependencies: &b_deps,
            },
        ];

        let opt = RustBuildOptions {
            mode: BuildMode::Test,
            package: Some("pkg_a".to_string()),
            ..Default::default()
        };

        let resolved =
            resolve_package_features_with_resolver(&packages, &opt, Some(resolver)).unwrap();
        let dep_feats = resolved
            .target
            .get("shared_dep@0.1.0")
            .cloned()
            .unwrap_or_default();
        assert!(
            dep_feats.contains(&"from_a".to_string()),
            "resolver {}: shared_dep must have from_a from root pkg_a",
            resolver
        );
        assert!(
            !dep_feats.contains(&"from_b".to_string()),
            "resolver {}: shared_dep must NOT have from_b from non-root pkg_b",
            resolver
        );
    }
}

#[test]
fn test_lib_test_false_doc_true_doctest_discovery_and_selection() {
    let toml: CargoToml = toml::from_str(
        r#"
        [package]
        name = "demo"
        version = "0.1.0"
        edition = "2021"

        [lib]
        test = false
        doc = true
        "#,
    )
    .unwrap();

    let temp = tempfile::tempdir().unwrap();
    let src_dir = temp.path().join("src");
    std::fs::create_dir_all(&src_dir).unwrap();
    std::fs::write(src_dir.join("lib.rs"), "//! doc").unwrap();

    // Doctest is enabled because doc=true and doctest defaults to true
    let doc_target = toml.discover_doctest_target(temp.path());
    assert!(
        doc_target.is_some(),
        "doctest must be discovered when test=false but doc=true"
    );

    // Test targets have LibUnit marked with explicit_only=true
    let test_targets = toml.discover_test_targets(temp.path());
    assert_eq!(test_targets.len(), 1);
    assert!(
        test_targets[0].explicit_only,
        "LibUnit must have explicit_only=true when lib.test=false"
    );

    // By default, select_test_targets excludes explicit_only targets
    let mut default_selected = test_targets.clone();
    select_test_targets(&mut default_selected, &RustBuildOptions::default());
    assert_eq!(
        default_selected.len(),
        0,
        "default test run must not execute lib unit test when test=false"
    );

    // But if --lib is explicitly passed, it IS selected
    let mut lib_selected = test_targets;
    select_test_targets(
        &mut lib_selected,
        &RustBuildOptions {
            lib_only: true,
            ..Default::default()
        },
    );
    assert_eq!(
        lib_selected.len(),
        1,
        "--lib must run lib unit test even if test=false"
    );
}

#[test]
fn test_lib_doc_false_disables_doctest_discovery() {
    let toml: CargoToml = toml::from_str(
        r#"
        [package]
        name = "demo"
        version = "0.1.0"
        edition = "2021"

        [lib]
        test = true
        doc = false
        "#,
    )
    .unwrap();

    let temp = tempfile::tempdir().unwrap();
    let src_dir = temp.path().join("src");
    std::fs::create_dir_all(&src_dir).unwrap();
    std::fs::write(src_dir.join("lib.rs"), "//! doc").unwrap();

    let doc_target = toml.discover_doctest_target(temp.path());
    assert!(
        doc_target.is_none(),
        "doctest must NOT be discovered when doc=false"
    );
}

#[test]
fn test_explicit_test_flag_runs_target_with_test_false() {
    let toml: CargoToml = toml::from_str(
        r#"
        [package]
        name = "demo"
        version = "0.1.0"
        edition = "2021"

        [[test]]
        name = "custom"
        test = false
        "#,
    )
    .unwrap();

    let temp = tempfile::tempdir().unwrap();
    let tests_dir = temp.path().join("tests");
    std::fs::create_dir_all(&tests_dir).unwrap();
    std::fs::write(tests_dir.join("custom.rs"), "fn main() {}").unwrap();

    let test_targets = toml.discover_test_targets(temp.path());
    let custom = test_targets
        .iter()
        .find(|t| t.name == "custom")
        .expect("custom test discovered");
    assert!(custom.explicit_only);

    // Validation succeeds when user requests --test custom
    let opt_req = RustBuildOptions {
        test_targets: vec!["custom".to_string()],
        ..Default::default()
    };
    let validation = validate_test_selection(&opt_req, &test_targets, &[]);
    assert!(
        validation.is_ok(),
        "validate_test_selection must accept explicit --test custom"
    );

    // select_test_targets retains custom when explicitly requested
    let mut selected = test_targets;
    select_test_targets(&mut selected, &opt_req);
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].name, "custom");
}

#[test]
fn test_doctest_command_native_search_link_args() {
    use mitts::adapter::rust::doctest::DoctestInvocation;
    use mitts::cli::test::TestArgs;
    use mitts::cli::test_doctest::doctest_command;
    use std::path::{Path, PathBuf};

    let inv = DoctestInvocation {
        crate_name: "my_lib".to_string(),
        crate_type: "lib".to_string(),
        src_path: PathBuf::from("src/lib.rs"),
        package_dir: PathBuf::from("/pkg"),
        edition: "2021".to_string(),
        search_dirs: vec![PathBuf::from("/target/deps")],
        native_search: vec!["native=/custom/native/dir".to_string()],
        ..Default::default()
    };
    let test_args = TestArgs::default();
    let rustdoc = Path::new("rustdoc");
    let cmd = doctest_command(rustdoc, &inv, &test_args);
    let args: Vec<String> = cmd
        .get_args()
        .map(|s| s.to_string_lossy().to_string())
        .collect();

    // Check that search_dirs has dependency= prefix
    assert!(
        args.contains(&"dependency=/target/deps".to_string()),
        "extern search dirs must have dependency= prefix: {:?}",
        args
    );
    // Check that native_search keeps kind prefix native=
    assert!(
        args.contains(&"native=/custom/native/dir".to_string()),
        "native search dirs must preserve kind prefix: {:?}",
        args
    );
}

#[test]
fn test_integration_test_discovery_includes_underscore_files() {
    let tmp = tempfile::tempdir().unwrap();
    let pkg_dir = tmp.path();
    std::fs::write(
        pkg_dir.join("Cargo.toml"),
        r#"
        [package]
        name = "underscore-demo"
        version = "0.1.0"
        edition = "2021"
        "#,
    )
    .unwrap();
    std::fs::create_dir_all(pkg_dir.join("tests")).unwrap();
    std::fs::write(
        pkg_dir.join("tests").join("_require_full.rs"),
        "fn main() {}",
    )
    .unwrap();
    std::fs::write(pkg_dir.join("tests").join("normal.rs"), "fn main() {}").unwrap();
    std::fs::write(pkg_dir.join("tests").join(".hidden.rs"), "fn main() {}").unwrap();

    let toml: CargoToml = toml::from_str(
        r#"
        [package]
        name = "underscore-demo"
        version = "0.1.0"
        edition = "2021"
        "#,
    )
    .unwrap();

    let targets = toml.discover_test_targets(pkg_dir);
    let names: Vec<String> = targets.into_iter().map(|t| t.name).collect();
    assert!(
        names.contains(&"_require_full".to_string()),
        "tests/_require_full.rs must be discovered as integration test: {:?}",
        names
    );
    assert!(
        names.contains(&"normal".to_string()),
        "tests/normal.rs must be discovered: {:?}",
        names
    );
    assert!(
        !names.contains(&".hidden".to_string()),
        "hidden files must be ignored: {:?}",
        names
    );
}
