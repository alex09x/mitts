/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! `mitts test -c` / `--concurrency[=N]` runs every test in a process of its own, many at a
//! time, the way cargo-nextest does. Without a number the pool is all the cores.

use mitts::cli::coverage_tools::resolve_llvm_tool;
use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use tempfile::TempDir;

/// A coverage run with `-c`: each test is a process with its own profile file, and the report
/// is the merge of all of them.
#[test]
fn coverage_with_concurrency_merges_the_profiles_of_all_test_processes() {
    if resolve_llvm_tool("llvm-profdata").is_none() || resolve_llvm_tool("llvm-cov").is_none() {
        eprintln!("skipped: llvm-tools are not installed");
        return;
    }
    let dir = TempDir::new().unwrap();
    write(
        &dir.path().join("Cargo.toml"),
        "[package]\nname = \"covered\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    // alpha (line 1) and beta (line 2) are each covered by their own test; gamma (line 3) by none.
    write(
        &dir.path().join("src/lib.rs"),
        "pub fn alpha() -> u32 { 1 }\npub fn beta() -> u32 { 2 }\npub fn gamma() -> u32 { 3 }\n\
         #[cfg(test)]\nmod tests {\n    use super::*;\n    #[test] fn a() { assert_eq!(alpha(), 1); }\n    #[test] fn b() { assert_eq!(beta(), 2); }\n}\n",
    );
    let report = dir.path().join("report.lcov");
    let output = Command::new(env!("CARGO_BIN_EXE_mitts"))
        .current_dir(dir.path())
        .args([
            "llvm-cov",
            "--offline",
            "--lib",
            "-c=3",
            "--lcov",
            "--output-path",
        ])
        .arg(&report)
        .arg("--cache-dir")
        .arg(dir.path().join("cache"))
        .env("CARGO_HOME", dir.path().join("cargo-home"))
        .env_remove("RUSTC_WRAPPER")
        .env_remove("CARGO_BUILD_RUSTC_WRAPPER")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let lcov = fs::read_to_string(&report).unwrap();
    let count = |line: u32| -> Option<u64> {
        lcov.lines()
            .find_map(|l| l.strip_prefix(&format!("DA:{line},")))
            .and_then(|c| c.split(',').next()?.parse().ok())
    };
    assert!(count(1).is_some_and(|c| c > 0), "alpha is covered:\n{lcov}");
    assert!(count(2).is_some_and(|c| c > 0), "beta is covered:\n{lcov}");
    assert_eq!(count(3), Some(0), "gamma is not covered:\n{lcov}");
}

fn write(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

const LIB: &str = r#"
use std::{thread::sleep, time::Duration};
fn nap() { sleep(Duration::from_millis(1000)); }

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn quick_one() {}
    #[test] fn quick_two() {}
    #[test] fn slow_a() { nap(); }
    #[test] fn slow_b() { nap(); }
    #[test] fn slow_c() { nap(); }
    #[test] fn slow_d() { nap(); }
    #[test] fn slow_e() { nap(); }
    #[test] fn slow_f() { nap(); }
    #[test] fn fails_on_purpose() { panic!("boom from the test"); }
    #[test] #[ignore] fn ignored_one() {}
}
"#;

fn project() -> TempDir {
    let dir = TempDir::new().unwrap();
    write(
        &dir.path().join("Cargo.toml"),
        "[package]\nname = \"many\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write(&dir.path().join("src/lib.rs"), LIB);
    dir
}

fn test(dir: &TempDir, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mitts"))
        .current_dir(dir.path())
        .arg("test")
        .arg("--offline")
        .arg("--lib")
        .arg("--cache-dir")
        .arg(dir.path().join("cache"))
        .args(args)
        .env("CARGO_HOME", dir.path().join("cargo-home"))
        .env_remove("RUSTC_WRAPPER")
        .env_remove("CARGO_BUILD_RUSTC_WRAPPER")
        .output()
        .unwrap()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// The seconds in the `Summary [  1.234s]` line.
fn summary_seconds(text: &str) -> f64 {
    let rest = text.split("Summary [").nth(1).expect("a summary line");
    rest.split("s]").next().unwrap().trim().parse().unwrap()
}

#[test]
fn tests_run_many_at_a_time_and_a_number_sets_how_many() {
    let dir = project();
    // Six tests of one second each: six at once take about a second, one at a time six.
    let wide = test(&dir, &["--concurrency=6", "slow_"]);
    assert!(wide.status.success(), "{}", stdout(&wide));
    assert!(
        stdout(&wide).contains("6 tests run: 6 passed, 0 failed"),
        "{}",
        stdout(&wide)
    );
    assert!(summary_seconds(&stdout(&wide)) < 4.0, "{}", stdout(&wide));

    let narrow = test(&dir, &["-c=1", "slow_"]);
    assert!(narrow.status.success(), "{}", stdout(&narrow));
    assert!(
        summary_seconds(&stdout(&narrow)) >= 5.5,
        "{}",
        stdout(&narrow)
    );
}

#[test]
fn the_flag_without_a_number_leaves_the_filter_alone() {
    let dir = project();
    let output = test(&dir, &["-c", "quick"]);
    assert!(output.status.success(), "{}", stdout(&output));
    assert!(
        stdout(&output).contains("2 tests run: 2 passed"),
        "{}",
        stdout(&output)
    );
}

#[test]
fn a_failing_test_fails_the_run_and_shows_its_output() {
    let dir = project();
    let output = test(&dir, &["-c=2", "fails_on_purpose"]);
    assert_eq!(output.status.code(), Some(101), "{}", stdout(&output));
    let text = stdout(&output);
    assert!(text.contains("FAIL"), "{text}");
    assert!(text.contains("boom from the test"), "{text}");
    assert!(text.contains("1 failed"), "{text}");
}

#[test]
fn ignored_tests_are_skipped_unless_asked_for() {
    let dir = project();
    let skipped = test(&dir, &["-c=2", "ignored_one"]);
    assert!(skipped.status.success(), "{}", stdout(&skipped));
    assert!(
        stdout(&skipped).contains("no tests matched (1 skipped)"),
        "{}",
        stdout(&skipped)
    );

    let asked = test(&dir, &["-c=2", "--ignored", "ignored_one"]);
    assert!(asked.status.success(), "{}", stdout(&asked));
    assert!(
        stdout(&asked).contains("1 tests run: 1 passed"),
        "{}",
        stdout(&asked)
    );
}

#[test]
fn without_the_flag_the_usual_runner_is_used() {
    let dir = project();
    let output = test(&dir, &["quick"]);
    assert!(output.status.success(), "{}", stdout(&output));
    let text = stdout(&output);
    assert!(text.contains("test result: ok"), "{text}");
    assert!(!text.contains("Summary ["), "{text}");
}
