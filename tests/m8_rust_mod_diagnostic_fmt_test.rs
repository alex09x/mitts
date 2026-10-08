/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use miette::{Diagnostic, NamedSource, SourceSpan};
use mitts::adapter::{RustAdapter, RustBuildOptions};
use mitts::cli::diagnostic::{render_compiler_diagnostics, CompilerDiagnostic};
use mitts::cli::fmt::{handle_fmt, FmtArgs};
use std::fs;
use std::path::PathBuf;

#[test]
fn test_m8_compiler_diagnostic_miette_methods_and_render() {
    let diag = CompilerDiagnostic {
        header: "Test error header".to_string(),
        code: Some("mitts::test::err".to_string()),
        src: Some(NamedSource::new("src/main.rs", "fn main() {}".to_string())),
        span: Some(SourceSpan::from(0..5)),
        label: Some("main fn".to_string()),
        help: Some("Consider checking syntax".to_string()),
    };

    assert_eq!(diag.code().unwrap().to_string(), "mitts::test::err");
    assert_eq!(diag.severity(), Some(miette::Severity::Error));
    assert_eq!(diag.help().unwrap().to_string(), "Consider checking syntax");
    assert!(diag.source_code().is_some());
    assert!(diag.labels().is_some());

    // Diagnostic without span/label
    let diag_simple = CompilerDiagnostic {
        header: "Simple error".to_string(),
        code: None,
        src: None,
        span: None,
        label: None,
        help: None,
    };
    assert!(diag_simple.code().is_none());
    assert!(diag_simple.help().is_none());
    assert!(diag_simple.source_code().is_none());
    assert!(diag_simple.labels().is_none());

    // render_compiler_diagnostics with various inputs
    render_compiler_diagnostics("", false);
    render_compiler_diagnostics("simple raw stderr without unit", true);

    let structured_stderr = r#"
=== Compilation failure for demo_pkg@0.1.0 [bin:demo] ===
Command: rustc src/main.rs
Working Directory: /tmp
error[E0425]: cannot find value `val` in this scope
 --> src/main.rs:12:5
"#;
    render_compiler_diagnostics(structured_stderr, true);
}

#[test]
fn test_m8_rust_adapter_manifest_resolution_and_defaults() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    // 1. Missing manifest
    let opt_missing = RustBuildOptions {
        manifest_path: Some(root.join("nonexistent/Cargo.toml")),
        ..Default::default()
    };
    assert!(RustAdapter::resolve_manifest(&opt_missing).is_err());

    // 2. Valid manifest and default target dir resolution
    let cargo_toml = root.join("Cargo.toml");
    fs::write(
        &cargo_toml,
        r#"[package]
name = "adapter-target-test"
version = "0.1.0"
edition = "2021"
"#,
    )
    .unwrap();

    let opt_valid = RustBuildOptions {
        manifest_path: Some(cargo_toml.clone()),
        target_dir: None,
        ..Default::default()
    };
    let resolved = RustAdapter::resolve_manifest(&opt_valid).unwrap();
    assert_eq!(resolved, cargo_toml);

    let target_dir = RustAdapter::resolve_target_dir(&cargo_toml, &opt_valid).unwrap();
    assert_eq!(target_dir, root.join("target").join("mitts"));
}

#[test]
fn test_m8_fmt_cli_errors_and_empty_packages() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    // 1. Missing manifest for fmt
    let fmt_missing = FmtArgs {
        manifest_path: Some(root.join("nonexistent/Cargo.toml")),
        ..Default::default()
    };
    assert_eq!(handle_fmt(&fmt_missing), 1);

    // 2. Broken toml syntax
    let broken_toml = root.join("Cargo.toml");
    fs::write(&broken_toml, b"[package\ninvalid").unwrap();
    let fmt_broken = FmtArgs {
        manifest_path: Some(broken_toml),
        ..Default::default()
    };
    assert_eq!(handle_fmt(&fmt_broken), 1);

    // 3. Package with no source files
    let empty_crate = root.join("empty_crate");
    fs::create_dir_all(&empty_crate).unwrap();
    fs::write(
        empty_crate.join("Cargo.toml"),
        r#"[package]
name = "empty_crate"
version = "0.1.0"
edition = "2021"
"#,
    )
    .unwrap();
    let fmt_empty = FmtArgs {
        manifest_path: Some(empty_crate.join("Cargo.toml")),
        ..Default::default()
    };
    assert_eq!(handle_fmt(&fmt_empty), 0);

    // 4. Running fmt --check on hello-rust fixture
    let fixture_manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("hello-rust")
        .join("Cargo.toml");
    let fmt_fixture = FmtArgs {
        manifest_path: Some(fixture_manifest),
        check: true,
        verbose: 1,
        ..Default::default()
    };
    let exit_code = handle_fmt(&fmt_fixture);
    assert_eq!(exit_code, 0);
}
