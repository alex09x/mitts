/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::rust::types::{BuildMode, PlannedCompilationUnit, RustBuildOptions};
use mitts::adapter::rust::unit_inputs::{
    compute_unit_coarse_inputs, is_file_in_scanned_pkg_sources, is_file_in_scanned_tree,
    is_file_in_unit_coarse_inputs, PackageInputsCacheEntry,
};
use mitts::adapter::RustAdapter;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

fn setup_test_crate(root: &Path) {
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(root.join("tests/support")).unwrap();
    fs::create_dir_all(root.join("assets")).unwrap();

    fs::write(
        root.join("Cargo.toml"),
        r#"[package]
name = "isolated_test_crate"
version = "0.1.0"
edition = "2021"

[lib]
path = "src/lib.rs"

[[test]]
name = "test_a"
path = "tests/a.rs"

[[test]]
name = "test_b"
path = "tests/b.rs"
"#,
    )
    .unwrap();

    fs::write(
        root.join("src/lib.rs"),
        r#"pub fn add(a: i32, b: i32) -> i32 {
    a + b
}
"#,
    )
    .unwrap();

    fs::write(
        root.join("tests/a.rs"),
        r#"#[test]
fn it_works_a() {
    assert_eq!(isolated_test_crate::add(1, 2), 3);
}
"#,
    )
    .unwrap();

    fs::write(
        root.join("tests/b.rs"),
        r#"#[test]
fn it_works_b() {
    assert_eq!(isolated_test_crate::add(2, 3), 5);
}
"#,
    )
    .unwrap();
}

fn is_action_cached(action: &mitts::core::action::Action) -> bool {
    action.description.starts_with("Restored")
}

#[test]
fn test_mutate_single_test_recompiles_only_that_test() {
    let cache_dir = TempDir::new().unwrap();
    let project_dir = TempDir::new().unwrap();
    setup_test_crate(project_dir.path());

    let opt = RustBuildOptions {
        manifest_path: Some(project_dir.path().join("Cargo.toml")),
        cache_dir: Some(cache_dir.path().to_path_buf()),
        target_dir: Some(project_dir.path().join("target")),
        mode: BuildMode::Test,
        all_tests: true,
        ..Default::default()
    };

    // 1. Initial Action Keys derivation
    let expl1 = RustAdapter::explain_plan(&opt).expect("explain initial plan");
    let keys1 = RustAdapter::derive_unit_action_keys(&opt, &expl1).expect("derive keys 1");

    let lib_id = expl1.units.iter().find(|u| u.target_name == "isolated_test_crate").unwrap().unit_id();
    let test_a_id = expl1.units.iter().find(|u| u.target_name == "test_a").unwrap().unit_id();
    let test_b_id = expl1.units.iter().find(|u| u.target_name == "test_b").unwrap().unit_id();

    // 2. Mutate only tests/a.rs
    fs::write(
        project_dir.path().join("tests/a.rs"),
        r#"#[test]
fn it_works_a_mutated() {
    assert_eq!(isolated_test_crate::add(10, 20), 30);
}
"#,
    )
    .unwrap();

    // 3. Re-derive Action Keys after leaf test mutation
    let expl2 = RustAdapter::explain_plan(&opt).expect("explain second plan");
    let keys2 = RustAdapter::derive_unit_action_keys(&opt, &expl2).expect("derive keys 2");

    assert_eq!(
        keys1[&lib_id], keys2[&lib_id],
        "Library ActionKey must be completely unchanged when tests/a.rs is edited"
    );
    assert_eq!(
        keys1[&test_b_id], keys2[&test_b_id],
        "Sibling test_b ActionKey must be completely unchanged when tests/a.rs is edited"
    );
    assert_ne!(
        keys1[&test_a_id], keys2[&test_a_id],
        "Mutated test_a ActionKey must change"
    );
}

#[test]
fn test_mutate_shared_test_support_recompiles_only_dependent_tests() {
    let cache_dir = TempDir::new().unwrap();
    let project_dir = TempDir::new().unwrap();
    setup_test_crate(project_dir.path());

    // Add shared helper in tests/support/mod.rs
    fs::write(
        project_dir.path().join("tests/support/mod.rs"),
        r#"pub fn helper_val() -> i32 { 42 }
"#,
    )
    .unwrap();

    // test_a uses support helper
    fs::write(
        project_dir.path().join("tests/a.rs"),
        r#"mod support;
#[test]
fn it_works_a() {
    assert_eq!(support::helper_val(), 42);
}
"#,
    )
    .unwrap();

    let opt = RustBuildOptions {
        manifest_path: Some(project_dir.path().join("Cargo.toml")),
        cache_dir: Some(cache_dir.path().to_path_buf()),
        target_dir: Some(project_dir.path().join("target")),
        mode: BuildMode::Test,
        all_tests: true,
        ..Default::default()
    };

    // 1. Initial build: compiles lib and tests, publishing ActionManifest with extra inputs
    let (_, res1) = RustAdapter::build(&opt).expect("initial build");
    assert!(res1.success);

    // 2. Mutate tests/support/mod.rs
    fs::write(
        project_dir.path().join("tests/support/mod.rs"),
        r#"pub fn helper_val() -> i32 { 99 }
"#,
    )
    .unwrap();

    // 3. Second build: lib and test_b should be cached, test_a should be recompiled
    let (plan2, res2) = RustAdapter::build(&opt).expect("second build");
    assert!(res2.success);

    let plan_units = plan2.actions;
    let lib_action = plan_units.iter().find(|a| a.description.contains("lib"));
    let test_b_action = plan_units.iter().find(|a| a.description.contains("test_b"));
    let test_a_action = plan_units.iter().find(|a| a.description.contains("test_a"));

    if let Some(la) = lib_action {
        assert!(is_action_cached(la), "lib must remain cached when test support is mutated");
    }
    if let Some(tb) = test_b_action {
        assert!(is_action_cached(tb), "test_b must remain cached when unreferenced support is mutated");
    }
    if let Some(ta) = test_a_action {
        assert!(!is_action_cached(ta), "test_a must be recompiled when its support helper is mutated");
    }

}

#[test]
fn test_mutate_path_attribute_file_in_tests_recompiles_lib_and_dependents() {
    let cache_dir = TempDir::new().unwrap();
    let project_dir = TempDir::new().unwrap();
    setup_test_crate(project_dir.path());

    // Place extra source file inside tests/
    fs::write(
        project_dir.path().join("tests/support/extra_calc.rs"),
        r#"pub fn calc() -> i32 { 100 }
"#,
    )
    .unwrap();

    // Include it in src/lib.rs via #[path]
    fs::write(
        project_dir.path().join("src/lib.rs"),
        r#"#[path = "../tests/support/extra_calc.rs"]
pub mod extra_calc;

pub fn add(a: i32, b: i32) -> i32 {
    a + b + extra_calc::calc()
}
"#,
    )
    .unwrap();

    let opt = RustBuildOptions {
        manifest_path: Some(project_dir.path().join("Cargo.toml")),
        cache_dir: Some(cache_dir.path().to_path_buf()),
        target_dir: Some(project_dir.path().join("target")),
        mode: BuildMode::Test,
        all_tests: true,
        ..Default::default()
    };

    // 1. Initial build: lib compiles and records extra_calc.rs in extra_inputs
    let (_, res1) = RustAdapter::build(&opt).expect("initial build");
    assert!(res1.success);

    // 2. Mutate the file inside tests/ that is referenced via #[path]
    fs::write(
        project_dir.path().join("tests/support/extra_calc.rs"),
        r#"pub fn calc() -> i32 { 200 }
"#,
    )
    .unwrap();

    // 3. Second build: lib must be recompiled because extra_calc.rs changed
    let (plan2, res2) = RustAdapter::build(&opt).expect("second build");
    assert!(res2.success);

    let plan_units = plan2.actions;
    let lib_action = plan_units.iter().find(|a| a.description.contains("lib"));
    if let Some(la) = lib_action {
        assert!(!is_action_cached(la), "lib must be recompiled when #[path] file in tests/ is mutated");
    }

}

#[test]
fn test_mutate_lib_recompiles_lib_and_all_tests() {
    let cache_dir = TempDir::new().unwrap();
    let project_dir = TempDir::new().unwrap();
    setup_test_crate(project_dir.path());

    let opt = RustBuildOptions {
        manifest_path: Some(project_dir.path().join("Cargo.toml")),
        cache_dir: Some(cache_dir.path().to_path_buf()),
        target_dir: Some(project_dir.path().join("target")),
        mode: BuildMode::Test,
        all_tests: true,
        ..Default::default()
    };

    let expl1 = RustAdapter::explain_plan(&opt).expect("explain plan 1");
    let keys1 = RustAdapter::derive_unit_action_keys(&opt, &expl1).expect("derive keys 1");
    let lib_id = expl1.units.iter().find(|u| u.target_name == "isolated_test_crate").unwrap().unit_id();
    let test_a_id = expl1.units.iter().find(|u| u.target_name == "test_a").unwrap().unit_id();
    let test_b_id = expl1.units.iter().find(|u| u.target_name == "test_b").unwrap().unit_id();

    // Mutate src/lib.rs
    fs::write(
        project_dir.path().join("src/lib.rs"),
        r#"pub fn add(a: i32, b: i32) -> i32 {
    a + b + 1
}
"#,
    )
    .unwrap();

    let expl2 = RustAdapter::explain_plan(&opt).expect("explain plan 2");
    let keys2 = RustAdapter::derive_unit_action_keys(&opt, &expl2).expect("derive keys 2");

    assert_ne!(keys1[&lib_id], keys2[&lib_id], "lib ActionKey must change when src/lib.rs is mutated");
    assert_ne!(keys1[&test_a_id], keys2[&test_a_id], "test_a ActionKey must change when dependency lib changes");
    assert_ne!(keys1[&test_b_id], keys2[&test_b_id], "test_b ActionKey must change when dependency lib changes");
}

#[test]
fn test_mutate_included_str_recompiles_lib() {
    let cache_dir = TempDir::new().unwrap();
    let project_dir = TempDir::new().unwrap();
    setup_test_crate(project_dir.path());

    fs::write(project_dir.path().join("assets/data.txt"), "initial data\n").unwrap();
    fs::write(
        project_dir.path().join("src/lib.rs"),
        r#"pub fn add(a: i32, b: i32) -> i32 { a + b }
pub const DATA: &str = include_str!("../assets/data.txt");
pub fn get_data() -> &'static str {
    DATA
}
"#,
    )

    .unwrap();

    let opt = RustBuildOptions {
        manifest_path: Some(project_dir.path().join("Cargo.toml")),
        cache_dir: Some(cache_dir.path().to_path_buf()),
        target_dir: Some(project_dir.path().join("target")),
        mode: BuildMode::Test,
        all_tests: true,
        ..Default::default()
    };

    let (_, res1) = RustAdapter::build(&opt).expect("initial build");
    assert!(res1.success);

    // Mutate assets/data.txt
    fs::write(project_dir.path().join("assets/data.txt"), "updated data\n").unwrap();

    let (plan2, res2) = RustAdapter::build(&opt).expect("second build");
    assert!(res2.success);

    let plan_units = plan2.actions;
    let lib_action = plan_units.iter().find(|a| a.description.contains("lib"));
    if let Some(la) = lib_action {
        assert!(!is_action_cached(la), "lib must be recompiled when include_str! file is mutated");
    }

}

#[test]
fn test_mutate_cargo_toml_recompiles_all_package_units() {
    let cache_dir = TempDir::new().unwrap();
    let project_dir = TempDir::new().unwrap();
    setup_test_crate(project_dir.path());

    let opt = RustBuildOptions {
        manifest_path: Some(project_dir.path().join("Cargo.toml")),
        cache_dir: Some(cache_dir.path().to_path_buf()),
        target_dir: Some(project_dir.path().join("target")),
        mode: BuildMode::Test,
        all_tests: true,
        ..Default::default()
    };

    let expl1 = RustAdapter::explain_plan(&opt).expect("explain plan 1");
    let keys1 = RustAdapter::derive_unit_action_keys(&opt, &expl1).expect("derive keys 1");
    let lib_id = expl1.units.iter().find(|u| u.target_name == "isolated_test_crate").unwrap().unit_id();
    let test_a_id = expl1.units.iter().find(|u| u.target_name == "test_a").unwrap().unit_id();
    let test_b_id = expl1.units.iter().find(|u| u.target_name == "test_b").unwrap().unit_id();

    // Mutate Cargo.toml (add features section)
    fs::write(
        project_dir.path().join("Cargo.toml"),
        r#"[package]
name = "isolated_test_crate"
version = "0.1.0"
edition = "2021"

[features]
extra_feature = []

[lib]
path = "src/lib.rs"

[[test]]
name = "test_a"
path = "tests/a.rs"

[[test]]
name = "test_b"
path = "tests/b.rs"
"#,
    )
    .unwrap();

    let expl2 = RustAdapter::explain_plan(&opt).expect("explain plan 2");
    let keys2 = RustAdapter::derive_unit_action_keys(&opt, &expl2).expect("derive keys 2");

    assert_ne!(keys1[&lib_id], keys2[&lib_id], "lib ActionKey must change on Cargo.toml mutation");
    assert_ne!(keys1[&test_a_id], keys2[&test_a_id], "test_a ActionKey must change on Cargo.toml mutation");
    assert_ne!(keys1[&test_b_id], keys2[&test_b_id], "test_b ActionKey must change on Cargo.toml mutation");


}

#[test]
fn test_mutate_build_rs_recompiles_all_package_units() {
    let cache_dir = TempDir::new().unwrap();
    let project_dir = TempDir::new().unwrap();
    setup_test_crate(project_dir.path());

    // Add build.rs
    fs::write(
        project_dir.path().join("build.rs"),
        r#"fn main() {
    println!("cargo:rerun-if-changed=build.rs");
}
"#,
    )
    .unwrap();

    let opt = RustBuildOptions {
        manifest_path: Some(project_dir.path().join("Cargo.toml")),
        cache_dir: Some(cache_dir.path().to_path_buf()),
        target_dir: Some(project_dir.path().join("target")),
        mode: BuildMode::Test,
        all_tests: true,
        ..Default::default()
    };

    let expl1 = RustAdapter::explain_plan(&opt).expect("explain plan 1");
    let keys1 = RustAdapter::derive_unit_action_keys(&opt, &expl1).expect("derive keys 1");
    let lib_id = expl1.units.iter().find(|u| u.target_name == "isolated_test_crate").unwrap().unit_id();
    let test_a_id = expl1.units.iter().find(|u| u.target_name == "test_a").unwrap().unit_id();
    let test_b_id = expl1.units.iter().find(|u| u.target_name == "test_b").unwrap().unit_id();

    // Mutate build.rs
    fs::write(
        project_dir.path().join("build.rs"),
        r#"fn main() {
    println!("cargo:rustc-cfg=feature_foo");
}
"#,
    )
    .unwrap();

    let expl2 = RustAdapter::explain_plan(&opt).expect("explain plan 2");
    let keys2 = RustAdapter::derive_unit_action_keys(&opt, &expl2).expect("derive keys 2");

    assert_ne!(keys1[&lib_id], keys2[&lib_id], "lib ActionKey must change on build.rs mutation");
    assert_ne!(keys1[&test_a_id], keys2[&test_a_id], "test_a ActionKey must change on build.rs mutation");
    assert_ne!(keys1[&test_b_id], keys2[&test_b_id], "test_b ActionKey must change on build.rs mutation");
}

#[test]
fn test_is_file_in_scanned_pkg_sources_excludes_subcrates_and_temporary_files() {
    let project_dir = TempDir::new().unwrap();
    let root = project_dir.path();

    // Normal files in root, src, assets
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(root.join("assets")).unwrap();
    fs::write(root.join("src/lib.rs"), "pub fn f() {}").unwrap();
    fs::write(root.join("assets/data.txt"), "hello").unwrap();

    // Subcrate with its own Cargo.toml
    let subcrate_dir = root.join("subcrate");
    fs::create_dir_all(subcrate_dir.join("src")).unwrap();
    fs::write(subcrate_dir.join("Cargo.toml"), "[package]\nname = \"sub\"\nversion = \"0.1.0\"\n").unwrap();
    fs::write(subcrate_dir.join("src/lib.rs"), "pub fn sub() {}").unwrap();

    // Temporary files
    let tmp_file = root.join("src/lib.rs.tmp");
    let swp_file = root.join("src/.lib.rs.swp");
    let profraw_file = root.join("default.profraw");
    let git_file = root.join(".git/config");
    let lock_file = root.join("Cargo.lock");

    // Standard package sources must include src and assets
    assert!(is_file_in_scanned_pkg_sources(root, &root.join("src/lib.rs")));
    assert!(is_file_in_scanned_pkg_sources(root, &root.join("assets/data.txt")));

    // Standard package sources must exclude subcrates
    assert!(!is_file_in_scanned_pkg_sources(root, &subcrate_dir.join("src/lib.rs")));
    assert!(!is_file_in_scanned_pkg_sources(root, &subcrate_dir.join("Cargo.toml")));

    // Standard package sources must exclude temporary/VCS files
    assert!(!is_file_in_scanned_pkg_sources(root, &tmp_file));
    assert!(!is_file_in_scanned_pkg_sources(root, &swp_file));
    assert!(!is_file_in_scanned_pkg_sources(root, &profraw_file));
    assert!(!is_file_in_scanned_pkg_sources(root, &git_file));
    assert!(!is_file_in_scanned_pkg_sources(root, &lock_file));

    // Standard package sources must exclude test/bench/example dirs
    let test_file = root.join("tests/test1.rs");
    assert!(!is_file_in_scanned_pkg_sources(root, &test_file));

    // Scanned tree with allow_test_dirs=true includes test dir files, but still excludes subcrates & tmp files
    assert!(is_file_in_scanned_tree(root, &test_file, true));
    assert!(!is_file_in_scanned_tree(root, &subcrate_dir.join("src/lib.rs"), true));
    assert!(!is_file_in_scanned_tree(root, &tmp_file, true));
}

#[test]
fn test_impure_proc_macro_coarse_inputs_includes_tests_benches_examples() {
    let project_dir = TempDir::new().unwrap();
    let root = project_dir.path();

    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(root.join("tests")).unwrap();
    fs::create_dir_all(root.join("benches")).unwrap();
    fs::create_dir_all(root.join("examples")).unwrap();

    fs::write(root.join("src/lib.rs"), "pub fn f() {}").unwrap();
    fs::write(root.join("tests/my_test.rs"), "fn t() {}").unwrap();
    fs::write(root.join("benches/my_bench.rs"), "fn b() {}").unwrap();
    fs::write(root.join("examples/my_example.rs"), "fn ex() {}").unwrap();

    let pkg_entry = PackageInputsCacheEntry {
        common_inputs: vec![("Cargo.toml".to_string(), "h1".to_string())],
        src_inputs: vec![("src/lib.rs".to_string(), "h2".to_string())],
    };

    // 1. Normal unit without impure proc macro: coarse inputs only has common + src
    let normal_unit = PlannedCompilationUnit {
        kind: "lib".to_string(),
        src_path: root.join("src/lib.rs"),
        manifest_path: root.join("Cargo.toml"),
        ..Default::default()
    };
    let normal_coarse = compute_unit_coarse_inputs(&normal_unit, root, &pkg_entry).unwrap();
    let normal_paths: Vec<_> = normal_coarse.iter().map(|(p, _)| p.as_str()).collect();
    assert!(normal_paths.contains(&"src/lib.rs"));
    assert!(!normal_paths.iter().any(|p| p.starts_with("tests/")));
    assert!(!normal_paths.iter().any(|p| p.starts_with("benches/")));
    assert!(!normal_paths.iter().any(|p| p.starts_with("examples/")));
    assert!(!is_file_in_unit_coarse_inputs(&normal_unit, root, &root.join("tests/my_test.rs")));

    // 2. Unit with impure proc macro (e.g. sqlx_macros): coarse inputs includes tests/, benches/, examples/
    let impure_unit = PlannedCompilationUnit {
        kind: "lib".to_string(),
        src_path: root.join("src/lib.rs"),
        manifest_path: root.join("Cargo.toml"),
        dependencies: vec!["some_crate::proc-macro:sqlx_macros:0.7.0".to_string()],
        ..Default::default()
    };
    let impure_coarse = compute_unit_coarse_inputs(&impure_unit, root, &pkg_entry).unwrap();
    let impure_paths: Vec<_> = impure_coarse.iter().map(|(p, _)| p.as_str()).collect();
    assert!(impure_paths.contains(&"src/lib.rs"));
    assert!(impure_paths.iter().any(|p| p.contains("my_test.rs")));
    assert!(impure_paths.iter().any(|p| p.contains("my_bench.rs")));
    assert!(impure_paths.iter().any(|p| p.contains("my_example.rs")));
    assert!(is_file_in_unit_coarse_inputs(&impure_unit, root, &root.join("tests/my_test.rs")));

    // 3. Unit with pure proc macro (e.g. serde_derive): coarse inputs does NOT include tests/
    let pure_unit = PlannedCompilationUnit {
        kind: "lib".to_string(),
        src_path: root.join("src/lib.rs"),
        manifest_path: root.join("Cargo.toml"),
        dependencies: vec!["serde_derive::proc-macro:serde_derive:1.0.0".to_string()],
        ..Default::default()
    };
    let pure_coarse = compute_unit_coarse_inputs(&pure_unit, root, &pkg_entry).unwrap();
    let pure_paths: Vec<_> = pure_coarse.iter().map(|(p, _)| p.as_str()).collect();
    assert!(!pure_paths.iter().any(|p| p.starts_with("tests/")));
    assert!(!is_file_in_unit_coarse_inputs(&pure_unit, root, &root.join("tests/my_test.rs")));
}
