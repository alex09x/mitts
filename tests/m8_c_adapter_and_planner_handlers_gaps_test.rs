/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::c::{CAdapter, CBuildOptions};
use mitts::cli::diagnostic::render_compiler_diagnostics;
use mitts::cli::handlers::{handle_fetch, handle_gc};
use mitts::cli::{FetchArgs, GcArgs};
use std::fs;
use tempfile::TempDir;

#[test]
fn test_m8_c_adapter_manifest_resolution_and_default_dirs() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    // 1. Directory containing Mitts.toml
    let proj_dir = root.join("c_project");
    fs::create_dir_all(proj_dir.join("src")).unwrap();
    let manifest_file = proj_dir.join("Mitts.toml");
    fs::write(
        &manifest_file,
        "[package]\nname = \"my_c_lib\"\nversion = \"0.1.0\"\n\n[bin]\nsrc = \"src/main.c\"\n",
    )
    .unwrap();
    fs::write(
        proj_dir.join("src/main.c"),
        "int main(void) { return 0; }\n",
    )
    .unwrap();

    let opt_dir = CBuildOptions {
        manifest_path: Some(proj_dir.clone()),
        ..Default::default()
    };
    let resolved = CAdapter::resolve_manifest(&opt_dir).unwrap();
    assert_eq!(resolved, manifest_file);

    // 2. Non-existent path returns error
    let opt_none = CBuildOptions {
        manifest_path: Some(root.join("does_not_exist")),
        ..Default::default()
    };
    assert!(CAdapter::resolve_manifest(&opt_none).is_err());

    // 3. Build with target_dir = None and cache_dir = None (exercises default directories)
    let opt_defaults = CBuildOptions {
        manifest_path: Some(manifest_file),
        target_dir: None,
        cache_dir: None,
        ..Default::default()
    };
    let build_res = CAdapter::build(&opt_defaults);
    assert!(build_res.is_ok());
    let (plan, res) = build_res.unwrap();
    assert!(res.success);
    assert!(!plan.actions.is_empty());
}

#[test]
fn test_m8_diagnostic_fallback_and_verbose_rendering() {
    // 1. Non-standard error output without "error: " or standard rustc markers
    // Exercises CompilerDiagnostic fallback in cli/diagnostic.rs
    let non_standard_err = "fatal compiler bug encountered while compiling crate";
    render_compiler_diagnostics(non_standard_err, false);

    // 2. Structured error with command and cwd in verbose mode
    let structured_err = "error[E0425]: cannot find value `x` in this scope\n --> src/main.rs:10:5\n  |\n10|     x\n  |     ^\n\n--- compilation command ---\nrustc --crate-name test src/main.rs\n--- working directory ---\n/path/to/project\n";
    render_compiler_diagnostics(structured_err, true);
}

#[test]
fn test_m8_handlers_error_branches_with_invalid_cache() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    // 1. handle_fetch with nonexistent manifest
    let fetch_args = FetchArgs {
        manifest_path: Some(root.join("nonexistent_Cargo.toml")),
        ..Default::default()
    };
    let fetch_code = handle_fetch(&fetch_args);
    assert_eq!(fetch_code, 1);

    // 2. handle_gc with invalid cache_dir pointing to a file instead of directory
    let blocking_file = root.join("blocking_file.txt");
    fs::write(&blocking_file, "blocking").unwrap();

    let gc_args = GcArgs {
        cache_dir: Some(blocking_file),
        ..Default::default()
    };
    let gc_code = handle_gc(&gc_args);
    assert_eq!(gc_code, 1);
}
