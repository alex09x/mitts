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
use std::fs;

#[test]
fn test_m8_manifest_discover_multiple_bins_and_integration_tests() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let pkg_dir = temp_dir.path();

    let src_dir = pkg_dir.join("src");
    let bin_dir = src_dir.join("bin");
    let tests_dir = pkg_dir.join("tests");
    fs::create_dir_all(&bin_dir).unwrap();
    fs::create_dir_all(&tests_dir).unwrap();

    fs::write(src_dir.join("main.rs"), "fn main() {}\n").unwrap();
    fs::write(bin_dir.join("tool_a.rs"), "fn main() {}\n").unwrap();
    fs::write(bin_dir.join("tool_b.rs"), "fn main() {}\n").unwrap();

    fs::write(tests_dir.join("integration_one.rs"), "#[test] fn t1() {}\n").unwrap();
    fs::write(tests_dir.join("integration_two.rs"), "#[test] fn t2() {}\n").unwrap();

    let toml_str = r#"
[package]
name = "multi-target-pkg"
version = "0.2.0"
edition = "2021"
"#;
    let cargo_toml: CargoToml = toml::from_str(toml_str).expect("parse toml");
    let targets = cargo_toml.discover_targets(pkg_dir);

    let bin_targets: Vec<&str> = targets
        .iter()
        .filter(|t| t.kind == "bin")
        .map(|t| t.name.as_str())
        .collect();
    assert!(bin_targets.contains(&"multi-target-pkg"));
    assert!(bin_targets.contains(&"tool_a"));
    assert!(bin_targets.contains(&"tool_b"));

    let test_targets = cargo_toml.discover_test_targets(pkg_dir);
    let test_names: Vec<&str> = test_targets.iter().map(|t| t.name.as_str()).collect();
    assert!(test_names.contains(&"integration_one"));
    assert!(test_names.contains(&"integration_two"));
}

#[test]
fn test_m8_manifest_build_script_nested_and_disabled() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let pkg_dir = temp_dir.path();

    // 1. Default build.rs
    fs::write(pkg_dir.join("build.rs"), "fn main() {}\n").unwrap();

    let toml_default: CargoToml = toml::from_str(
        r#"
[package]
name = "standard-build"
version = "0.1.0"
"#,
    )
    .unwrap();
    assert!(toml_default.has_build_script(pkg_dir));

    // 2. Explicitly disabled build = false
    let toml_disabled: CargoToml = toml::from_str(
        r#"
[package]
name = "disabled-build"
version = "0.1.0"
build = false
"#,
    )
    .unwrap();
    assert!(!toml_disabled.has_build_script(pkg_dir));

    // 3. Custom path build = "custom.rs"
    let toml_custom: CargoToml = toml::from_str(
        r#"
[package]
name = "custom-build"
version = "0.1.0"
build = "custom.rs"
"#,
    )
    .unwrap();
    fs::write(pkg_dir.join("custom.rs"), "fn main() {}\n").unwrap();
    assert!(toml_custom.has_build_script(pkg_dir));
}

#[test]
fn test_m8_manifest_explicit_bin_and_test_sections() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let pkg_dir = temp_dir.path();
    let src_dir = pkg_dir.join("src");
    fs::create_dir_all(&src_dir).unwrap();
    fs::write(src_dir.join("custom_entry.rs"), "fn main() {}\n").unwrap();
    fs::write(src_dir.join("custom_test.rs"), "#[test] fn t() {}\n").unwrap();

    let toml_str = r#"
[package]
name = "explicit-pkg"
version = "0.1.0"

[[bin]]
name = "custom-bin"
path = "src/custom_entry.rs"
test = false

[[test]]
name = "custom-test"
path = "src/custom_test.rs"
"#;
    let cargo_toml: CargoToml = toml::from_str(toml_str).unwrap();
    let targets = cargo_toml.discover_targets(pkg_dir);
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].name, "custom-bin");

    let test_targets = cargo_toml.discover_test_targets(pkg_dir);
    assert_eq!(test_targets.len(), 1);
    assert_eq!(test_targets[0].name, "custom-test");
}

#[test]
fn test_m8_manifest_discover_directory_style_autobin() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let pkg_dir = temp_dir.path();
    let src_dir = pkg_dir.join("src");
    let bin_dir = src_dir.join("bin");
    let tool_dir = bin_dir.join("complex_tool");
    fs::create_dir_all(&tool_dir).unwrap();
    fs::write(src_dir.join("lib.rs"), "pub fn lib() {}\n").unwrap();
    fs::write(tool_dir.join("main.rs"), "fn main() {}\n").unwrap();

    let toml_str = r#"
[package]
name = "complex-pkg"
version = "0.1.0"
"#;
    let cargo_toml: CargoToml = toml::from_str(toml_str).unwrap();
    let targets = cargo_toml.discover_targets(pkg_dir);

    let bin_names: Vec<&str> = targets
        .iter()
        .filter(|t| t.kind == "bin")
        .map(|t| t.name.as_str())
        .collect();
    assert!(bin_names.contains(&"complex_tool"));
}
