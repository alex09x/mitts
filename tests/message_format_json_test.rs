/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! `mitts check|clippy --message-format=json` prints Cargo-shaped JSON records on stdout and
//! nothing else there: one `compiler-message` per diagnostic, then `build-finished`.

use serde_json::Value;
use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use tempfile::TempDir;

const FAILING: &str = "pub fn f() -> u8 { \"not a u8\" }\n";
const WARNING: &str = "pub fn f() { let unused = 1; }\n";
const CLEAN: &str = "pub fn f() -> u8 { 1 }\n";
const CLIPPY_LINT: &str = "pub fn f(x: bool) -> bool { if x { true } else { false } }\n";

fn crate_dir(lib: &str) -> TempDir {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"jsondemo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(dir.path().join("src/lib.rs"), lib).unwrap();
    dir
}

/// Runs the real CLI in `root` with an explicit cache, so the default store is never used.
fn mitts(root: &Path, cache: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mitts"))
        .current_dir(root)
        .args(args)
        .args(["--offline", "--cache-dir"])
        .arg(cache)
        .arg("--target-dir")
        .arg(root.join("target"))
        .env("CARGO_HOME", root.join("cargo-home"))
        .env_remove("RUSTC_WRAPPER")
        .env_remove("RUSTC_WORKSPACE_WRAPPER")
        .env_remove("CARGO_BUILD_RUSTC_WRAPPER")
        .env_remove("CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER")
        .output()
        .unwrap()
}

/// Every stdout line parsed as JSON; a line that is not JSON fails the test.
fn records(output: &Output) -> Vec<Value> {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str(line)
                .unwrap_or_else(|err| panic!("stdout line is not JSON ({err}): {line}"))
        })
        .collect()
}

fn diagnostics(records: &[Value]) -> Vec<&Value> {
    records
        .iter()
        .filter(|record| record["reason"] == "compiler-message")
        .collect()
}

fn diagnostic_with_code<'a>(records: &'a [Value], code: &str) -> &'a Value {
    diagnostics(records)
        .into_iter()
        .find(|record| record["message"]["code"]["code"] == code)
        .unwrap_or_else(|| panic!("no diagnostic with code {code} in {records:#?}"))
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
}

#[test]
fn failing_crate_reports_the_error_then_build_finished_false() {
    let dir = crate_dir(FAILING);
    let cache = TempDir::new().unwrap();
    let output = mitts(
        dir.path(),
        cache.path(),
        &["check", "--message-format=json"],
    );
    assert!(!output.status.success(), "a type error fails the check");

    let all = records(&output);
    let last = all.last().expect("at least the closing record");
    assert_eq!(last["reason"], "build-finished");
    assert_eq!(last["success"], false);

    let record = diagnostic_with_code(&all, "E0308");
    assert_eq!(record["message"]["level"], "error");
    assert!(record["message"]["rendered"]
        .as_str()
        .unwrap()
        .contains("E0308"));
    let span = &record["message"]["spans"][0];
    assert!(span["file_name"].as_str().unwrap().ends_with("lib.rs"));
    assert!(span["line_start"].as_u64().unwrap() >= 1);
    assert_eq!(record["target"]["name"], "jsondemo");
    assert_eq!(record["target"]["kind"][0], "lib");
    assert!(!record["package_id"].as_str().unwrap().is_empty());
    assert!(record["manifest_path"]
        .as_str()
        .unwrap()
        .ends_with("Cargo.toml"));
}

#[test]
fn warning_crate_reports_a_warning_and_succeeds() {
    let dir = crate_dir(WARNING);
    let cache = TempDir::new().unwrap();
    let output = mitts(
        dir.path(),
        cache.path(),
        &["check", "--message-format=json"],
    );
    assert!(output.status.success(), "{}", stderr(&output));

    let all = records(&output);
    let warning = diagnostic_with_code(&all, "unused_variables");
    assert_eq!(warning["message"]["level"], "warning");
    let last = all.last().unwrap();
    assert_eq!(last["reason"], "build-finished");
    assert_eq!(last["success"], true);
}

#[test]
fn clean_crate_prints_only_build_finished() {
    let dir = crate_dir(CLEAN);
    let cache = TempDir::new().unwrap();
    let output = mitts(
        dir.path(),
        cache.path(),
        &["check", "--message-format", "json"],
    );
    assert!(output.status.success(), "{}", stderr(&output));
    let all = records(&output);
    assert_eq!(all.len(), 1, "no banner or progress on stdout: {all:?}");
    assert_eq!(all[0]["reason"], "build-finished");
    assert_eq!(all[0]["success"], true);
}

#[test]
fn clippy_reports_lints_in_the_same_format() {
    let dir = crate_dir(CLIPPY_LINT);
    let cache = TempDir::new().unwrap();
    let output = mitts(
        dir.path(),
        cache.path(),
        &["clippy", "--message-format=json"],
    );
    assert!(output.status.success(), "{}", stderr(&output));
    let all = records(&output);
    let lint = diagnostic_with_code(&all, "clippy::needless_bool");
    assert_eq!(lint["message"]["level"], "warning");
    assert_eq!(all.last().unwrap()["reason"], "build-finished");
}

#[test]
fn cached_units_replay_their_diagnostics_as_records() {
    let dir = crate_dir(WARNING);
    let cache = TempDir::new().unwrap();
    let first = mitts(
        dir.path(),
        cache.path(),
        &["check", "--message-format=json"],
    );
    assert!(first.status.success());

    let second = mitts(
        dir.path(),
        cache.path(),
        &["check", "--message-format=json"],
    );
    assert!(second.status.success(), "{}", stderr(&second));
    assert!(
        stderr(&second).contains("compiled=0"),
        "the second run restores from the cache: {}",
        stderr(&second)
    );
    let all = records(&second);
    diagnostic_with_code(&all, "unused_variables");
    assert_eq!(all.last().unwrap()["success"], true);
}

#[test]
fn human_and_json_runs_never_replay_each_others_output() {
    let dir = crate_dir(WARNING);
    let cache = TempDir::new().unwrap();

    let human = mitts(dir.path(), cache.path(), &["check"]);
    assert!(human.status.success());
    assert!(
        stderr(&human).contains("unused variable"),
        "human text on stderr: {}",
        stderr(&human)
    );

    let json = mitts(
        dir.path(),
        cache.path(),
        &["check", "--message-format=json"],
    );
    assert!(json.status.success(), "{}", stderr(&json));
    let all = records(&json);
    diagnostic_with_code(&all, "unused_variables");

    let human_again = mitts(dir.path(), cache.path(), &["check"]);
    assert!(human_again.status.success());
    assert!(stderr(&human_again).contains("unused variable"));
    assert!(
        !String::from_utf8_lossy(&human_again.stdout).contains("{\"reason\":"),
        "human mode prints no JSON records"
    );
}

#[test]
fn unsupported_value_is_a_usage_error() {
    let dir = crate_dir(CLEAN);
    let cache = TempDir::new().unwrap();
    let output = mitts(dir.path(), cache.path(), &["check", "--message-format=xml"]);
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("message-format"),
        "{}",
        stderr(&output)
    );
    assert!(
        output.stdout.is_empty(),
        "nothing on stdout for a usage error"
    );
}
