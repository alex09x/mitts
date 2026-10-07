/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::rust::build_script::BuildScriptDirectives;
use mitts::adapter::rust::env_macro::{discover_package_env_macros, scan_env_macros};
use mitts::adapter::rust::manifest::CargoToml;
use mitts::adapter::rust::types::EnvMacroDiscovery;
use std::fs;

#[test]
fn test_m8_build_script_directives_comprehensive_parsing() {
    let stdout = r#"
cargo:rustc-link-lib=static=foo
cargo:rustc-link-search=native=/usr/local/lib
cargo:rustc-cfg=has_feature
cargo:rustc-env=MY_VAR=my_value
cargo:rustc-flags=-l dylib=bar
cargo:rerun-if-changed=build.rs
cargo:rerun-if-env-changed=MY_BUILD_ENV
cargo:warning=Custom compiler warning
cargo:custom_key=custom_value
"#;

    let mut directives = BuildScriptDirectives::parse_stdout(stdout);
    directives.links = Some("mylinks".to_string());

    assert_eq!(directives.link_libs, vec!["static=foo".to_string()]);
    assert_eq!(directives.link_searches, vec!["/usr/local/lib".to_string()]);
    assert_eq!(directives.cfgs, vec!["has_feature".to_string()]);
    assert_eq!(directives.envs.get("MY_VAR"), Some(&"my_value".to_string()));
    assert!(!directives.is_empty());
    assert_eq!(
        directives.metadata.get("custom_key"),
        Some(&"custom_value".to_string())
    );

    let dep_vars = directives.dep_env_vars();
    assert_eq!(dep_vars.get("DEP_MYLINKS_CUSTOM_KEY"), Some(&"custom_value".to_string()));

    // Test without links
    directives.links = None;
    let dep_vars_no_links = directives.dep_env_vars();
    assert!(dep_vars_no_links.is_empty());
}

#[test]
fn test_m8_manifest_discover_test_targets_and_bins() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(root.join("tests")).unwrap();
    fs::write(root.join("src/lib.rs"), b"pub fn f() {}").unwrap();
    fs::write(root.join("src/main.rs"), b"fn main() {}").unwrap();
    fs::write(root.join("tests/integration_one.rs"), b"#[test] fn t1() {}").unwrap();
    fs::write(root.join("tests/integration_two.rs"), b"#[test] fn t2() {}").unwrap();

    let manifest_content = r#"[package]
name = "full-targets-pkg"
version = "0.1.0"
edition = "2021"

[[test]]
name = "custom_test"
path = "tests/integration_one.rs"
"#;
    let toml: CargoToml = toml::from_str(manifest_content).unwrap();

    let targets = toml.discover_test_targets(root);
    assert!(targets.iter().any(|t| t.name == "full_targets_pkg_unit_test"));
    assert!(targets.iter().any(|t| t.name == "full_targets_pkg_bin_test"));
    assert!(targets.iter().any(|t| t.name == "custom_test"));
}

#[test]
fn test_m8_env_macro_raw_strings_and_dynamic_tokens() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::create_dir_all(root.join("src")).unwrap();

    // 1. Raw string literals in env! and option_env!
    let src1 = r###"
fn test_raw() {
    let _ = env!(r#"RAW_VAR_1"#);
    let _ = option_env!(r##"RAW_VAR_2"##);
    let _ = env!["BRACKET_VAR"];
    let _ = env!{"BRACE_VAR"};
}
"###;
    fs::write(root.join("src/lib.rs"), src1).unwrap();

    let (disc1, vars1) = discover_package_env_macros(root);
    assert!(matches!(disc1, EnvMacroDiscovery::Tracked(_)));
    assert!(vars1.contains(&"RAW_VAR_1".to_string()));
    assert!(vars1.contains(&"RAW_VAR_2".to_string()));
    assert!(vars1.contains(&"BRACKET_VAR".to_string()));
    assert!(vars1.contains(&"BRACE_VAR".to_string()));

    // 2. Dynamic untracked token inside env!
    let src2 = r#"
fn test_dynamic() {
    let _ = env!(concat!("DYNAMIC_", "VAR"));
}
"#;
    let disc2 = scan_env_macros(src2);
    assert_eq!(disc2, EnvMacroDiscovery::UntrackedDynamic);
}
