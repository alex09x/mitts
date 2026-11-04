/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Coverage reports name every file by a path that identifies it, leave out dependency sources,
//! honor `--ignore-filename-regex` everywhere including the threshold, and can be written to a
//! file with `--output-path`.

use mitts::cli::coverage_tools::resolve_llvm_tool;
use serde_json::Value;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;

/// The crate under measurement, a path dependency that sits inside its directory and has its
/// own lockfile (so its own "workspace root" is itself), and one that sits outside it. All
/// three have a `src/lib.rs`.
struct Fixture {
    _dir: TempDir,
    root: PathBuf,
    cache: PathBuf,
}

fn fixture() -> Fixture {
    let dir = TempDir::new().unwrap();
    let root = dir.path().join("app");
    let write = |path: PathBuf, body: &str| {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    };
    write(
        root.join("Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
         [dependencies]\nvendored = { path = \"vendored/vendored\" }\n\
         outsider = { path = \"../outsider\" }\n",
    );
    write(
        root.join("src/lib.rs"),
        "pub fn covered() -> u32 { vendored::helper() + outsider::helper() + 1 }\n\
         pub fn uncovered() -> u32 { 2 }\n\n\
         #[cfg(test)]\nmod tests {\n    #[test]\n    fn covers() { assert_eq!(super::covered(), 3); }\n}\n",
    );
    for (path, name) in [
        (root.join("vendored/vendored"), "vendored"),
        (dir.path().join("outsider"), "outsider"),
    ] {
        write(
            path.join("Cargo.toml"),
            &format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
        );
        write(
            path.join("src/lib.rs"),
            "pub fn helper() -> u32 { 1 }\npub fn dead_code_one() -> u32 { 8 }\npub fn dead_code_two() -> u32 { 9 }\n",
        );
    }
    // A lockfile next to the vendored crate makes it its own workspace root.
    write(
        root.join("vendored/vendored/Cargo.lock"),
        "version = 3\n\n[[package]]\nname = \"vendored\"\nversion = \"0.1.0\"\n",
    );
    let cache = dir.path().join("cache");
    Fixture {
        _dir: dir,
        root,
        cache,
    }
}

fn llvm_tools_available() -> bool {
    resolve_llvm_tool("llvm-profdata").is_some() && resolve_llvm_tool("llvm-cov").is_some()
}

impl Fixture {
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_mitts"))
            .current_dir(&self.root)
            .arg("llvm-cov")
            .args(args)
            .args(["--offline", "--cache-dir"])
            .arg(&self.cache)
            .env("CARGO_HOME", self.root.join("cargo-home"))
            .output()
            .unwrap()
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).to_string()
}

fn lcov_files(lcov: &str) -> Vec<String> {
    lcov.lines()
        .filter_map(|line| line.strip_prefix("SF:"))
        .map(str::to_string)
        .collect()
}

fn json_files(report: &Value) -> BTreeSet<String> {
    report["data"][0]["files"]
        .as_array()
        .map(|files| {
            files
                .iter()
                .map(|file| file["filename"].as_str().unwrap().to_string())
                .collect()
        })
        .unwrap_or_default()
}

fn line_percent(report: &Value) -> f64 {
    report["data"][0]["totals"]["lines"]["percent"]
        .as_f64()
        .expect("totals.lines.percent")
}

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn report_paths_are_unique_and_identify_the_file() {
    if !llvm_tools_available() {
        eprintln!("skipped: llvm-tools are not installed");
        return;
    }
    let fixture = fixture();
    let lcov_path = fixture.root.join("out/coverage.lcov");
    let output = fixture.run(&["--lcov", "--output-path", lcov_path.to_str().unwrap()]);
    assert!(output.status.success(), "{}", text(&output.stderr));

    let files = lcov_files(&fs::read_to_string(&lcov_path).unwrap());
    for expected in [
        "src/lib.rs",
        "vendored/vendored/src/lib.rs",
        "external/outsider-0.1.0/src/lib.rs",
    ] {
        assert!(
            files.iter().any(|file| file == expected),
            "{expected} missing from {files:?}"
        );
    }
    let unique: BTreeSet<_> = files.iter().collect();
    assert_eq!(unique.len(), files.len(), "paths collide: {files:?}");
    for file in &files {
        assert!(
            !file.contains("/.cargo/") && !file.contains("/sources/") && !file.contains("/rustc/"),
            "dependency or toolchain source in the report: {file}"
        );
    }
}

#[test]
fn ignore_regex_applies_to_every_format_and_to_the_threshold() {
    if !llvm_tools_available() {
        eprintln!("skipped: llvm-tools are not installed");
        return;
    }
    let fixture = fixture();
    let all = fixture.root.join("out/all.json");
    let filtered = fixture.root.join("out/filtered.json");
    let regex = "(vendored/|external/)";

    let output = fixture.run(&[
        "--json",
        "--summary-only",
        "--output-path",
        all.to_str().unwrap(),
    ]);
    assert!(output.status.success(), "{}", text(&output.stderr));
    let output = fixture.run(&[
        "--json",
        "--summary-only",
        "--ignore-filename-regex",
        regex,
        "--output-path",
        filtered.to_str().unwrap(),
    ]);
    assert!(output.status.success(), "{}", text(&output.stderr));

    let all_report = read_json(&all);
    let filtered_report = read_json(&filtered);
    assert!(
        json_files(&all_report).len() >= 3,
        "{:?}",
        json_files(&all_report)
    );
    assert_eq!(
        json_files(&filtered_report),
        BTreeSet::from(["src/lib.rs".to_string()]),
        "only the measured crate is left"
    );
    let filtered_percent = line_percent(&filtered_report);
    assert!(
        (line_percent(&all_report) - filtered_percent).abs() > 0.01,
        "the dependencies change the total, so the filter matters"
    );
    assert!(
        filtered_percent > 1.0 && filtered_percent < 99.0,
        "{filtered_percent}"
    );

    // The same regex drives the threshold: it passes just below the filtered total and fails
    // just above it.
    let below = format!("{:.2}", filtered_percent - 0.5);
    let output = fixture.run(&[
        "--ignore-filename-regex",
        regex,
        "--fail-under-lines",
        &below,
    ]);
    assert!(output.status.success(), "{}", text(&output.stderr));
    let above = format!("{:.2}", filtered_percent + 0.5);
    let output = fixture.run(&[
        "--ignore-filename-regex",
        regex,
        "--fail-under-lines",
        &above,
    ]);
    assert!(!output.status.success(), "threshold {above} must fail");

    // LCOV honors it as well.
    let lcov = fixture.root.join("out/filtered.lcov");
    let output = fixture.run(&[
        "--lcov",
        "--ignore-filename-regex",
        regex,
        "--output-path",
        lcov.to_str().unwrap(),
    ]);
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        lcov_files(&fs::read_to_string(&lcov).unwrap()),
        ["src/lib.rs"]
    );
}

#[test]
fn output_path_writes_the_report_to_the_file_and_not_to_stdout() {
    if !llvm_tools_available() {
        eprintln!("skipped: llvm-tools are not installed");
        return;
    }
    let fixture = fixture();
    let report = fixture.root.join("target/nested/coverage-report.json");
    let output = fixture.run(&[
        "--workspace",
        "--json",
        "--summary-only",
        "--output-path",
        report.to_str().unwrap(),
    ]);
    assert!(output.status.success(), "{}", text(&output.stderr));
    let stdout = text(&output.stdout);
    assert!(
        !stdout.contains("\"data\""),
        "the JSON report must not reach stdout: {stdout}"
    );
    let parsed = read_json(&report);
    assert!(
        parsed["data"][0]["totals"]["lines"]["count"]
            .as_u64()
            .unwrap()
            > 0
    );
    assert!(
        parsed["data"][0]["files"][0].get("summary").is_some(),
        "--summary-only exports per-file summaries"
    );

    let text_report = fixture.root.join("out/report.txt");
    let output = fixture.run(&["--text", "--output-path", text_report.to_str().unwrap()]);
    assert!(output.status.success(), "{}", text(&output.stderr));
    let body = fs::read_to_string(&text_report).unwrap();
    assert!(
        body.contains("TOTAL") && !body.contains('\u{1b}'),
        "plain table: {body}"
    );
}

#[test]
fn output_path_needs_exactly_one_single_file_format() {
    let fixture = fixture();
    let target = fixture.root.join("out/x");
    let target = target.to_str().unwrap();
    for args in [
        vec!["--output-path", target],
        vec!["--html", "--output-path", target],
        vec!["--json", "--lcov", "--output-path", target],
    ] {
        let output = fixture.run(&args);
        assert_eq!(
            output.status.code(),
            Some(2),
            "{args:?}: {}",
            text(&output.stderr)
        );
        assert!(text(&output.stderr).contains("--output-path"), "{args:?}");
    }
}
