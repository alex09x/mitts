/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! `mitts llvm-cov --json` writes the JSON report to stdout, and a consumer parses stdout as one
//! JSON document. The banner, the stage lines and the output of the instrumented tests used to
//! reach stdout ahead of it (issue 44), so `json.loads` failed at the first character. In this
//! mode they go to stderr, the way cargo-llvm-cov keeps stdout for the report.

use mitts::cli::coverage_tools::resolve_llvm_tool;
use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use tempfile::TempDir;

fn write(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

fn llvm_tools_present() -> bool {
    resolve_llvm_tool("llvm-profdata").is_some() && resolve_llvm_tool("llvm-cov").is_some()
}

fn run(extra: &[&str]) -> (TempDir, Output) {
    let dir = TempDir::new().unwrap();
    let app = dir.path().join("app");
    write(
        &app.join("Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write(
        &app.join("src/lib.rs"),
        "pub fn f() -> u32 { println!(\"printed by the library\"); 1 }\n\n\
         #[cfg(test)]\nmod tests {\n    #[test]\n    fn t() { println!(\"printed by a test\"); assert_eq!(super::f(), 1); }\n}\n",
    );
    let output = Command::new(env!("CARGO_BIN_EXE_mitts"))
        .current_dir(&app)
        .args(["llvm-cov", "--all-targets"])
        .args(extra)
        .arg("--cache-dir")
        .arg(dir.path().join("cache"))
        .env("CARGO_HOME", dir.path().join("cargo-home"))
        .output()
        .unwrap();
    (dir, output)
}

fn assert_stdout_is_the_report(output: &Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let report: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|err| panic!("stdout is not one JSON document ({err}):\n{stdout}"));
    let files = report["data"][0]["files"]
        .as_array()
        .expect("data[0].files");
    assert!(files.iter().any(|f| f["filename"] == "src/lib.rs"));
    assert!(report["data"][0]["totals"]["lines"]["percent"].is_number());

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("running 1 test"),
        "the output of the test binaries is kept, on stderr:\n{stderr}"
    );
}

#[test]
fn json_summary_only_leaves_stdout_to_the_report() {
    if !llvm_tools_present() {
        eprintln!("skipped: llvm-tools are not installed");
        return;
    }
    let (_dir, output) = run(&["--json", "--summary-only"]);
    assert_stdout_is_the_report(&output);
}

#[test]
fn json_leaves_stdout_to_the_report() {
    if !llvm_tools_present() {
        eprintln!("skipped: llvm-tools are not installed");
        return;
    }
    let (_dir, output) = run(&["--json"]);
    assert_stdout_is_the_report(&output);
}

#[test]
fn json_to_a_file_keeps_the_console_output_on_stdout() {
    if !llvm_tools_present() {
        eprintln!("skipped: llvm-tools are not installed");
        return;
    }
    let dir = TempDir::new().unwrap();
    let report = dir.path().join("report.json");
    let (_app, output) = run(&["--json", "--output-path", report.to_str().unwrap()]);
    assert!(output.status.success());
    // The file is the report; stdout is the usual progress, which a consumer does not parse.
    assert!(String::from_utf8_lossy(&output.stdout).contains("running 1 test"));
    let written: serde_json::Value = serde_json::from_slice(&fs::read(&report).unwrap()).unwrap();
    assert!(written["data"][0]["files"].is_array());
}
