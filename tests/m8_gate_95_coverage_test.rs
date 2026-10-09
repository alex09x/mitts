/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::fs;
use std::path::Path;
use tempfile::TempDir;

use mitts::adapter::rust::compiler::RustcInvocation;
use mitts::adapter::rust::dep_info::{find_dep_info_files, parse_dep_info};
use mitts::adapter::rust::inputs::hash_package_base_inputs;
use mitts::adapter::rust::types::PlannedCompilationUnit;
use mitts::adapter::rust::unescape::unescape_rust_string;
use mitts::adapter::rust::{BuildMode, RustAdapter, RustBuildOptions};
use mitts::cli::coverage_tools::resolve_llvm_tool;
use mitts::cli::diagnostic::render_compiler_diagnostics;
use mitts::core::reflink::reflink_or_copy;

#[test]
fn test_m8_diagnostic_source_span_resolution() {
    let temp = TempDir::new().unwrap();
    let src_dir = temp.path().join("src");
    fs::create_dir_all(&src_dir).unwrap();
    let lib_rs = src_dir.join("lib.rs");
    fs::write(
        &lib_rs,
        "fn hello() {\n    let val: u32 = \"not a number\";\n    println!(\"{}\", val);\n}\n",
    )
    .unwrap();

    let stderr = format!(
        "rustc failed for unit 'demo@0.1.0::lib:demo' (exit code 1):\n  command: \"rustc\" \"--crate-name\" \"demo\"\n  cwd: {}\nerror[E0308]: mismatched types\n --> src/lib.rs:2:20\n  |\n2 |     let val: u32 = \"not a number\";\n  |                    ^^^^^^^^^^^^^^ expected `u32`, found `&str`\n  = help: remove the quotes\n",
        temp.path().display()
    );

    render_compiler_diagnostics(&stderr, false);
    render_compiler_diagnostics(&stderr, true);
}

#[test]
fn test_m8_inputs_immutable_registry_without_cargo_toml() {
    let temp = TempDir::new().unwrap();
    let pkg_dir = temp.path().join("immutable_no_toml");
    fs::create_dir_all(&pkg_dir).unwrap();
    fs::write(pkg_dir.join(".cargo-ok"), "ok").unwrap();

    let res = hash_package_base_inputs(&pkg_dir, None, false).unwrap();
    assert_eq!(res.len(), 2);
    assert_eq!(res[0].0, "Cargo.toml");
    assert_eq!(res[1].0, "immutable_pkg");
}

#[test]
fn test_m8_diagnostic_unstructured_and_missing_sections() {
    // 1. Diagnostics with section having no quotes in first line
    let text = "=== Compilation failed for unit raw_unit_without_quotes\nerror: something bad\n";
    render_compiler_diagnostics(text, false);

    // 2. Diagnostics completely empty
    render_compiler_diagnostics("", false);
}

#[test]
fn test_m8_coverage_tools_path_fallback() {
    let res = resolve_llvm_tool("nonexistent-tool-xyz-12345");
    assert!(res.is_none());
}

#[test]
fn test_m8_reflink_empty_parent() {
    let res = reflink_or_copy(Path::new("a"), Path::new(""), false);
    assert!(res.is_err());
}

#[test]
fn test_m8_inputs_nested_workspace_ancestor_and_cargo_config() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    // 1. Root workspace
    let ws_toml = root.join("Cargo.toml");
    fs::write(&ws_toml, "[workspace]\nmembers = [\"crates/*\"]\n").unwrap();
    let root_cargo_dir = root.join(".cargo");
    fs::create_dir_all(&root_cargo_dir).unwrap();
    fs::write(root_cargo_dir.join("config"), "[build]\njobs = 4\n").unwrap();

    // 2. Intermediate ancestor manifest
    let crates_dir = root.join("crates");
    fs::create_dir_all(&crates_dir).unwrap();
    let ancestor_toml = crates_dir.join("Cargo.toml");
    fs::write(
        &ancestor_toml,
        "[package]\nname = \"ancestor\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();

    // 3. Leaf package
    let pkg_dir = crates_dir.join("leaf");
    let src_dir = pkg_dir.join("src");
    fs::create_dir_all(&src_dir).unwrap();
    let leaf_toml = pkg_dir.join("Cargo.toml");
    fs::write(
        &leaf_toml,
        "[package]\nname = \"leaf\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(src_dir.join("lib.rs"), "pub fn leaf() {}\n").unwrap();
    let leaf_cargo_dir = pkg_dir.join(".cargo");
    fs::create_dir_all(&leaf_cargo_dir).unwrap();
    fs::write(leaf_cargo_dir.join("config"), "[target]\n").unwrap();

    let inputs = hash_package_base_inputs(&pkg_dir, Some(&ws_toml), true).unwrap();
    let keys: Vec<String> = inputs.into_iter().map(|(k, _)| k).collect();
    assert!(keys.iter().any(|k| k == "Cargo.toml"));
    assert!(keys.iter().any(|k| k == "workspace:Cargo.toml"));
    assert!(keys.iter().any(|k| k.starts_with("ancestor:")));
    assert!(keys.iter().any(|k| k == ".cargo/config"));
    assert!(keys.iter().any(|k| k == "workspace:.cargo/config"));
}

#[test]
fn test_m8_dep_info_direct_normalized_discovery() {
    let temp = TempDir::new().unwrap();
    let target_dir = temp.path().join("target");
    let debug_dir = target_dir.join("debug");
    fs::create_dir_all(&debug_dir).unwrap();
    let norm_file = debug_dir.join("my_crate.d");
    fs::write(&norm_file, "target: src/lib.rs\n").unwrap();

    let found = find_dep_info_files(&target_dir, "my-crate");
    assert!(found.iter().any(|f| f == &norm_file));
}

#[test]
fn test_m8_coverage_tools_missing_env_and_path_hit() {
    // 1. Missing env variable override
    std::env::set_var("MITTS_FAKE_TOOL_ENV_VAR", "missing");
    let res = resolve_llvm_tool("fake-tool-env-var");
    assert!(res.is_none());
    std::env::remove_var("MITTS_FAKE_TOOL_ENV_VAR");

    // 2. Tool that exists in PATH but not rustlib (e.g. sh)
    let res_sh = resolve_llvm_tool("sh");
    assert!(res_sh.is_some());
}

#[test]
fn test_m8_dep_info_deps_normalized_discovery() {
    let temp = TempDir::new().unwrap();
    let target_dir = temp.path().join("target");
    let deps_dir = target_dir.join("debug").join("deps");
    fs::create_dir_all(&deps_dir).unwrap();
    let norm_file = deps_dir.join("my_dash_pkg.d");
    fs::write(&norm_file, "target: src/lib.rs\n").unwrap();

    let found = find_dep_info_files(&target_dir, "my-dash-pkg");
    assert!(found.iter().any(|f| f == &norm_file));
}

#[test]
fn test_m8_compiler_test_no_dest_and_non_semver() {
    let temp = TempDir::new().unwrap();
    let src_dir = temp.path().join("src");
    fs::create_dir_all(&src_dir).unwrap();
    let src_file = src_dir.join("lib.rs");
    fs::write(&src_file, "pub fn foo() {}\n").unwrap();

    let out_dir = temp.path().join("out");
    fs::create_dir_all(&out_dir).unwrap();

    let unit = PlannedCompilationUnit {
        package_id: "custom@0.1.0".to_string(),
        package_name: "custom".to_string(),
        version: "custom-v1-patch".to_string(),
        target_name: "custom".to_string(),
        kind: "test".to_string(),
        manifest_path: temp.path().join("Cargo.toml"),
        src_path: src_file,
        cache_eligibility: "eligible".to_string(),
        edition: Some("2021".to_string()),
        is_workspace: true,
        is_primary: false,
        env_vars: Vec::new(),
        dependencies: Vec::new(),
        features: Vec::new(),
    };

    let inv = RustcInvocation {
        unit: &unit,
        action_key: "short_key_test_no_dest",
        out_dir: &out_dir,
        dest_binary: None,
        extern_crates: &[],
        release: false,
        features: &[],
        directives: None,
        mode: BuildMode::Test,
        extra_compiler_args: &[],
        jobserver_auth: None,
        deps_dir: None,
    };
    let res = inv.execute().unwrap();
    assert!(res.success);
}

#[test]
fn test_m8_builder_native_dag_compilation_failure() {
    let temp = TempDir::new().unwrap();
    let pkg_dir = temp.path().join("failing_syntax_pkg");
    let src_dir = pkg_dir.join("src");
    fs::create_dir_all(&src_dir).unwrap();
    fs::write(
        pkg_dir.join("Cargo.toml"),
        "[package]\nname = \"failing_syntax_pkg\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(src_dir.join("lib.rs"), "syntax error here !!!").unwrap();

    let opt = RustBuildOptions {
        manifest_path: Some(pkg_dir.join("Cargo.toml")),
        ..Default::default()
    };
    let res = RustAdapter::build_native(&opt);
    assert!(res.is_ok());
    let (_, action_res) = res.unwrap();
    assert!(!action_res.success);
}

#[test]
fn test_m8_unescape_rust_string_edge_cases() {
    assert_eq!(unescape_rust_string(r"\u1234"), None);
    assert_eq!(unescape_rust_string(r"\u{}"), None);
    assert_eq!(unescape_rust_string(r"\u{1234567}"), None);
    assert_eq!(unescape_rust_string(r"\x80"), None);
    assert_eq!(unescape_rust_string(r"\z"), None);
    assert_eq!(unescape_rust_string(r"\"), None);
    assert_eq!(unescape_rust_string(r"\x"), None);
    assert_eq!(unescape_rust_string(r"\xG1"), None);
    assert_eq!(unescape_rust_string(r"\u{D800}"), None);
    assert_eq!(unescape_rust_string(r"\u{G}"), None);
    assert_eq!(unescape_rust_string(r"\u{123"), None);
}

#[test]
fn test_m8_parse_dep_info_error_and_trailing_backslash() {
    // 1. Missing file error
    let missing = parse_dep_info(Path::new("/nonexistent/dep_info_file.d"));
    assert!(missing.is_err());

    // 2. Trailing backslash and lines without colon
    let temp = TempDir::new().unwrap();
    let dep_file = temp.path().join("dependency.rs");
    fs::write(&dep_file, "pub fn d() {}\n").unwrap();
    let d_file = temp.path().join("test.d");
    fs::write(
        &d_file,
        format!("target_without_colon \\\n  {}\n", dep_file.display()),
    )
    .unwrap();
    let files = parse_dep_info(&d_file).unwrap();
    assert_eq!(files.len(), 1);
}
