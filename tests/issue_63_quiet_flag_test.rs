/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Integration tests for issue #63: `-q` / `--quiet` across `test` (including `-c` parallel mode,
//! `--no-run`, and doctests), `check`, `clippy`, `bench`, and `coverage`.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use tempfile::TempDir;

fn mitts(root: &Path, cache: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mitts"))
        .current_dir(root)
        .args(args)
        .args(["--offline", "--cache-dir"])
        .arg(cache)
        .arg("--target-dir")
        .arg(root.join("target"))
        .env("CARGO_HOME", root.join("cargo-home"))
        .env_remove("MITTS_CACHE_DIR")
        .env_remove("RUSTC_WRAPPER")
        .env_remove("RUSTC_WORKSPACE_WRAPPER")
        .env_remove("CARGO_BUILD_RUSTC_WRAPPER")
        .env_remove("CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER")
        .output()
        .expect("run mitts")
}

fn write_crate(dir: &Path, lib_src: &str) {
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname = \"quietdemo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(dir.join("src/lib.rs"), lib_src).unwrap();
}

fn assert_no_mitts_banners(stdout: &str, stderr: &str, label: &str) {
    for needle in [
        "Mitts Build",
        "[mitts::stats]",
        "profile completed",
        "profile failed",
    ] {
        assert!(
            !stdout.contains(needle) && !stderr.contains(needle),
            "{label} leaked `{needle}`:\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
        );
    }
}

#[test]
fn test_quiet_flag_suppresses_banners_and_preserves_summary_and_failures() {
    let tmp = TempDir::new().unwrap();
    let cache = tmp.path().join("cache");
    let proj = tmp.path().join("proj");

    write_crate(
        &proj,
        r#"/// ```
/// assert_eq!(quietdemo::add(2, 3), 5);
/// ```
pub fn add(a: i32, b: i32) -> i32 {
    a + b
}

#[cfg(test)]
mod tests {
    #[test]
    fn t_one() {
        assert_eq!(super::add(1, 2), 3);
    }

    #[test]
    fn t_two() {
        assert_eq!(super::add(4, 5), 9);
    }
}
"#,
    );

    // 1. `mitts test -q` and `mitts test --quiet` on passing unit + doc tests.
    for flag in ["-q", "--quiet"] {
        let out = mitts(&proj, &cache, &["test", flag]);
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            out.status.success(),
            "test {flag} failed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
        );
        assert_no_mitts_banners(&stdout, &stderr, &format!("test {flag}"));
        assert!(
            !stderr.contains("Running ") && !stderr.contains("Doc-tests "),
            "test {flag} leaked target headers:\n{stderr}"
        );
        assert!(
            stdout.contains("test result: ok."),
            "test {flag} must preserve libtest summary:\n{stdout}"
        );
        assert!(
            !stdout.contains("test tests::t_one ... ok"),
            "test {flag} should pass -q to libtest (terse dots, not per-test lines):\n{stdout}"
        );
    }

    // 2. `mitts test -q --no-run` produces zero stdout/stderr chatter on success.
    let no_run = mitts(&proj, &cache, &["test", "-q", "--no-run"]);
    let nr_stdout = String::from_utf8_lossy(&no_run.stdout);
    let nr_stderr = String::from_utf8_lossy(&no_run.stderr);
    assert!(no_run.status.success());
    assert_eq!(
        nr_stdout.trim(),
        "",
        "test -q --no-run stdout must be empty"
    );
    assert_no_mitts_banners(&nr_stdout, &nr_stderr, "test -q --no-run");

    // 3. `mitts test -c -q` (parallel mode) suppresses PASS lines and banners, keeps Summary.
    let par_ok = mitts(&proj, &cache, &["test", "-c", "-q", "--lib"]);
    let par_stdout = String::from_utf8_lossy(&par_ok.stdout);
    let par_stderr = String::from_utf8_lossy(&par_ok.stderr);
    assert!(
        par_ok.status.success(),
        "test -c -q failed:\nstdout:\n{par_stdout}\nstderr:\n{par_stderr}"
    );
    assert_no_mitts_banners(&par_stdout, &par_stderr, "test -c -q");
    assert!(
        !par_stdout.contains("PASS") && !par_stderr.contains("PASS"),
        "test -c -q must suppress PASS lines:\nstdout:\n{par_stdout}\nstderr:\n{par_stderr}"
    );
    assert!(
        par_stdout.contains("Summary"),
        "test -c -q must keep Summary line:\nstdout:\n{par_stdout}\nstderr:\n{par_stderr}"
    );

    // 4. Failing test under `-q` and `-c -q`: preserves failure output and exit code 101.
    write_crate(
        &proj,
        r#"pub fn broken() -> i32 { 1 }
#[cfg(test)]
mod tests {
    #[test]
    fn passes() { assert_eq!(super::broken(), 1); }
    #[test]
    fn explodes() { panic!("boom_from_quiet_test"); }
}
"#,
    );

    let fail_q = mitts(&proj, &cache, &["test", "-q", "--lib"]);
    let fq_stdout = String::from_utf8_lossy(&fail_q.stdout);
    let fq_stderr = String::from_utf8_lossy(&fail_q.stderr);
    assert_eq!(fail_q.status.code(), Some(101));
    assert_no_mitts_banners(&fq_stdout, &fq_stderr, "failing test -q");
    assert!(
        fq_stdout.contains("boom_from_quiet_test") || fq_stderr.contains("boom_from_quiet_test"),
        "failing test -q must show panic message:\nstdout:\n{fq_stdout}\nstderr:\n{fq_stderr}"
    );
    assert!(fq_stdout.contains("test result: FAILED."));

    let fail_par_q = mitts(&proj, &cache, &["test", "-c", "-q", "--lib"]);
    let fpq_stdout = String::from_utf8_lossy(&fail_par_q.stdout);
    let fpq_stderr = String::from_utf8_lossy(&fail_par_q.stderr);
    assert_eq!(fail_par_q.status.code(), Some(101));
    assert_no_mitts_banners(&fpq_stdout, &fpq_stderr, "failing test -c -q");
    assert!(!fpq_stdout.contains("PASS") && !fpq_stderr.contains("PASS"));
    assert!(
        fpq_stdout.contains("FAIL"),
        "failing test -c -q must print FAIL:\n{fpq_stdout}"
    );
    assert!(
        fpq_stdout.contains("boom_from_quiet_test") || fpq_stderr.contains("boom_from_quiet_test"),
        "failing test -c -q must retain failure output:\nstdout:\n{fpq_stdout}\nstderr:\n{fpq_stderr}"
    );
}

#[test]
fn quiet_flag_accepted_on_check_clippy_bench_and_coverage() {
    let tmp = TempDir::new().unwrap();
    let cache = tmp.path().join("cache");
    let proj = tmp.path().join("proj_subcmds");
    write_crate(&proj, "pub fn clean_value() -> u32 { 7 }\n");

    for args in [
        &["check", "-q"][..],
        &["check", "--quiet"][..],
        &["clippy", "-q"][..],
        &["bench", "-q", "--no-run"][..],
        &["coverage", "-q", "--no-run"][..],
    ] {
        let out = mitts(&proj, &cache, args);
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            out.status.success(),
            "{args:?} failed (status {:?}):\nstdout:\n{stdout}\nstderr:\n{stderr}",
            out.status.code()
        );
        assert_eq!(stdout.trim(), "", "{args:?} must emit nothing on stdout");
        assert_no_mitts_banners(&stdout, &stderr, &format!("{args:?}"));
    }

    // Failing `check -q` preserves compiler diagnostics on stderr and exit code 101 without banners.
    write_crate(&proj, "pub fn bad() -> u32 { \"wrong_type\" }\n");
    let bad_check = mitts(&proj, &cache, &["check", "-q"]);
    let bc_stdout = String::from_utf8_lossy(&bad_check.stdout);
    let bc_stderr = String::from_utf8_lossy(&bad_check.stderr);
    assert_eq!(bad_check.status.code(), Some(101));
    assert_eq!(
        bc_stdout.trim(),
        "",
        "failing check -q stdout must be empty"
    );
    assert_no_mitts_banners(&bc_stdout, &bc_stderr, "failing check -q");
    assert!(
        bc_stderr.contains("E0308") || bc_stderr.contains("mismatched types"),
        "failing check -q must preserve compiler error:\n{bc_stderr}"
    );
}
