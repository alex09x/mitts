/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use miette::Diagnostic;
use mitts::cli::diagnostic::{render_compiler_diagnostics, CompilerDiagnostic};
use mitts::core::build_logger::BuildLogger;
use std::fs;
use std::time::Duration;

#[test]
fn test_m8_compiler_diagnostic_rendering_and_methods() {
    // 1. Diagnostic with all fields
    let diag = CompilerDiagnostic {
        header: "mismatched types".to_string(),
        code: Some("E0308".to_string()),
        src: None,
        span: None,
        label: Some("expected `i32`, found `&str`".to_string()),
        help: Some("try calling `.parse()`".to_string()),
    };

    assert_eq!(diag.code().unwrap().to_string(), "E0308");
    assert_eq!(diag.help().unwrap().to_string(), "try calling `.parse()`");
    assert_eq!(diag.severity(), Some(miette::Severity::Error));
    assert!(diag.source_code().is_none());
    assert!(diag.labels().is_none());

    // 2. render_compiler_diagnostics empty and fallback
    render_compiler_diagnostics("", false);
    render_compiler_diagnostics("   \n  \t", true);

    // Unstructured error
    render_compiler_diagnostics("error: link failure: cannot find -lmissing_lib\n", true);

    // Standard compiler diagnostic format
    let rustc_stderr = r#"
error[E0425]: cannot find value `foo` in this scope
 --> src/main.rs:2:5
  |
2 |     foo();
  |     ^^^ not found in this scope
  |
  = help: consider importing this function
"#;
    render_compiler_diagnostics(rustc_stderr, true);
    render_compiler_diagnostics(rustc_stderr, false);
}

#[test]
fn test_m8_build_logger_lifecycle_and_file_output() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let log_file = temp_dir.path().join("build.log");

    let mut logger = BuildLogger::new(Some(log_file.clone()));

    // Record phase directly
    logger.record_phase("planning", Duration::from_millis(15), "planned 4 units");

    // Measure closure
    let answer = logger.measure("compilation", "compiled 4 units", || {
        std::thread::sleep(Duration::from_millis(10));
        42
    });
    assert_eq!(answer, 42);

    // Start phase timer
    let timer = logger.start_phase("caching");
    timer.finish_with_details("cached 4 objects");

    assert!(logger.total_elapsed() >= Duration::from_millis(10));
    assert_eq!(logger.phases().len(), 3);

    // Write log to file
    logger.write_trace("dev", 0, "success").expect("write build log to file");
    assert!(log_file.is_file());

    let content = fs::read_to_string(&log_file).unwrap();
    assert!(content.contains("planning"));
    assert!(content.contains("compilation"));
    assert!(content.contains("caching"));
    assert!(content.contains("compiled 4 units"));
}
