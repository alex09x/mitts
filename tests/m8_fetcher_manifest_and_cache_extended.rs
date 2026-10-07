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
use mitts::adapter::rust::types::RustBuildOptions;
use mitts::adapter::RustAdapter;
use std::fs;
use tempfile::TempDir;

#[test]
fn test_m8_manifest_aliases_and_test_targets_extended() {
    let toml_str = r#"
[package]
name = "alias-pkg"
version = "0.2.0"
edition = "2021"
default-run = "bin_one"

[lib]
name = "custom_lib"
path = "src/custom_lib.rs"

[[bin]]
name = "bin_one"
path = "src/bin1.rs"

[[test]]
name = "integration_spec"
path = "tests/spec.rs"

[dependencies]
renamed_serde = { package = "serde", version = "1.0", features = ["derive"] }
renamed_syn = { package = "syn", version = "2.0" }
normal_dep = "0.4"
"#;
    let toml = CargoToml::parse(toml_str).unwrap();

    let aliases = toml.dependency_aliases();
    assert!(aliases.real_to_aliases.contains_key("serde"));
    let serde_aliases = &aliases.real_to_aliases["serde"];
    assert!(serde_aliases.iter().any(|(a, _)| a == "renamed_serde"));

    assert!(aliases.real_to_aliases.contains_key("syn"));
    let syn_aliases = &aliases.real_to_aliases["syn"];
    assert!(syn_aliases.iter().any(|(a, _)| a == "renamed_syn"));

    // Discover targets on directory
    let tmp = TempDir::new().unwrap();
    let src_dir = tmp.path().join("src");
    let tests_dir = tmp.path().join("tests");
    fs::create_dir_all(&src_dir).unwrap();
    fs::create_dir_all(&tests_dir).unwrap();

    fs::write(src_dir.join("custom_lib.rs"), "pub fn clib() {}\n").unwrap();
    fs::write(src_dir.join("bin1.rs"), "fn main() {}\n").unwrap();
    fs::write(tests_dir.join("spec.rs"), "#[test] fn s() {}\n").unwrap();

    let targets = toml.discover_targets(tmp.path());
    assert!(targets.iter().any(|t| t.name == "custom_lib" && t.kind == "lib"));
    assert!(targets.iter().any(|t| t.name == "bin_one" && t.kind == "bin"));

    let test_targets = toml.discover_test_targets(tmp.path());
    assert!(test_targets.iter().any(|t| t.name == "integration_spec" && t.kind == "test"));
    assert!(test_targets.iter().any(|t| t.name == "alias_pkg_unit_test" && t.kind == "test"));
}

#[test]
fn test_m8_dag_cache_restores_bin_and_rlib_non_native() {
    let tmp = TempDir::new().unwrap();
    let manifest = tmp.path().join("Cargo.toml");
    let src = tmp.path().join("src");
    fs::create_dir_all(&src).unwrap();

    fs::write(
        &manifest,
        "[package]\nname = \"cache-restore-pkg\"\nversion = \"0.1.0\"\n\n[lib]\npath = \"src/lib.rs\"\n\n[[bin]]\nname = \"restore_bin\"\npath = \"src/main.rs\"\n",
    )
    .unwrap();
    fs::write(src.join("lib.rs"), "pub fn msg() -> &'static str { \"restored\" }\n").unwrap();
    fs::write(
        src.join("main.rs"),
        "fn main() { println!(\"{}\", cache_restore_pkg::msg()); }\n",
    )
    .unwrap();

    let target_dir = tmp.path().join("target");
    let cache_dir = tmp.path().join("cache");

    // 1. Initial build: populates CAS and Action Cache
    let opt = RustBuildOptions {
        manifest_path: Some(manifest.clone()),
        target_dir: Some(target_dir.clone()),
        cache_dir: Some(cache_dir.clone()),
        native: false, // non-native triggers copy into target/debug/deps
        release: false,
        ..Default::default()
    };
    let (_plan1, res1) = RustAdapter::build(&opt).unwrap();
    assert!(res1.success);
    assert_eq!(res1.exit_code, 0);

    // Remove local target directory so fast-lock cannot short-circuit, forcing CAS cache restore
    fs::remove_dir_all(&target_dir).unwrap();

    // 2. Second build: restores rlib and bin from CAS
    let opt_restore = RustBuildOptions {
        manifest_path: Some(manifest),
        target_dir: Some(target_dir.clone()),
        cache_dir: Some(cache_dir),
        native: false,
        release: false,
        ..Default::default()
    };
    let (plan2, res2) = RustAdapter::build(&opt_restore).unwrap();
    assert!(res2.success);
    assert_eq!(res2.exit_code, 0);

    // Check that target/debug/restore_bin was restored
    let bin_path = target_dir.join("debug").join("restore_bin");
    assert!(bin_path.is_file(), "Binary must be restored from CAS into target/debug");
    assert!(!plan2.actions.is_empty());
}
