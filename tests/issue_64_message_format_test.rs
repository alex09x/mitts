/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Integration tests for issue #64: `--message-format` (`human`, `short`, `json`,
//! `json-diagnostic-short`, `json-diagnostic-rendered-ansi`, `json-render-diagnostics`,
//! and Cargo comma-separated modifier combinations) across `build`, `check`, `clippy`, and `test`.

use serde_json::Value;
use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use tempfile::TempDir;

fn mitts(root: &Path, cache: &Path, args: &[&str]) -> Output {
    mitts_with_env(root, cache, args, &[])
}

fn mitts_with_env(root: &Path, cache: &Path, args: &[&str], envs: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mitts"));
    cmd.current_dir(root)
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
        .env_remove("CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER");
    for (k, v) in envs {
        cmd.env(k, v);
    }
    cmd.output().expect("run mitts")
}

fn write_crate(dir: &Path, lib_src: &str) {
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname = \"fmtcrate\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(dir.join("src/lib.rs"), lib_src).unwrap();
}

fn parse_ndjson(stdout: &[u8]) -> Vec<Value> {
    String::from_utf8_lossy(stdout)
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|line| {
            serde_json::from_str(line)
                .unwrap_or_else(|e| panic!("stdout line is not valid JSON ({e}): {line}"))
        })
        .collect()
}

#[test]
fn short_format_works_across_check_build_clippy_and_test() {
    let tmp = TempDir::new().unwrap();
    let cache = tmp.path().join("cache");
    let proj = tmp.path().join("proj_short");

    write_crate(
        &proj,
        "pub fn f(x: bool) -> bool { let unused_var = 1; if x { true } else { false } }\n",
    );

    for subcmd in [
        &["check", "--message-format=short"][..],
        &["build", "--message-format=short"][..],
        &["clippy", "--message-format=short"][..],
        &["test", "--no-run", "--message-format=short"][..],
    ] {
        let out = mitts(&proj, &cache, subcmd);
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            out.status.success(),
            "{subcmd:?} failed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
        );
        assert_eq!(stdout.trim(), "", "{subcmd:?} stdout must be empty");
        assert!(
            stderr.contains("src/lib.rs:1:") && stderr.contains("warning:"),
            "{subcmd:?} stderr must contain single-line short diagnostic:\n{stderr}"
        );
        assert!(
            !stderr.contains("\n  --> ")
                && !stderr.contains("Mitts Build")
                && !stderr.contains("Command:"),
            "{subcmd:?} stderr must not contain multi-line snippet or Mitts banners:\n{stderr}"
        );
    }

    // Failing build in `short` format exits 101 without `Command:` or miette box.
    write_crate(&proj, "pub fn bad() -> u8 { \"nope\" }\n");
    let fail = mitts(&proj, &cache, &["check", "--message-format=short"]);
    let f_stdout = String::from_utf8_lossy(&fail.stdout);
    let f_stderr = String::from_utf8_lossy(&fail.stderr);
    assert_eq!(fail.status.code(), Some(101));
    assert_eq!(f_stdout.trim(), "");
    assert!(
        f_stderr.contains("src/lib.rs:1:") && f_stderr.contains("error[E0308]"),
        "short error missing:\n{f_stderr}"
    );
    assert!(
        !f_stderr.contains("Command:") && !f_stderr.contains("mitts::compiler::failed"),
        "short failure must not emit Command: prefix or miette wrapper:\n{f_stderr}"
    );
}

#[test]
fn cargo_json_variants_and_pipelined_build_test_emit_expected_streams() {
    let tmp = TempDir::new().unwrap();
    let cache = tmp.path().join("cache");
    let ws = tmp.path().join("ws_json");

    // Two-crate workspace so `dep_lib` is compiled with pipelined rustc during `build` and `test`.
    fs::create_dir_all(ws.join("dep_lib/src")).unwrap();
    fs::create_dir_all(ws.join("app/src")).unwrap();
    fs::write(
        ws.join("Cargo.toml"),
        "[workspace]\nresolver = \"2\"\nmembers = [\"dep_lib\", \"app\"]\n",
    )
    .unwrap();
    fs::write(
        ws.join("dep_lib/Cargo.toml"),
        "[package]\nname = \"dep_lib\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(
        ws.join("dep_lib/src/lib.rs"),
        "pub fn val() -> u32 { let unused_in_dep = 9; 5 }\n",
    )
    .unwrap();
    fs::write(
        ws.join("app/Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\ndep_lib = { path = \"../dep_lib\" }\n",
    )
    .unwrap();
    fs::write(
        ws.join("app/src/lib.rs"),
        "pub fn run() -> u32 { dep_lib::val() }\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn ok() { assert_eq!(super::run(), 5); }\n}\n",
    )
    .unwrap();

    // 1. `build -p app --message-format=json` and `test --workspace --no-run --message-format=json` (pipelined dep_lib!).
    for args in [
        &["build", "-p", "app", "--message-format=json"][..],
        &["test", "--workspace", "--no-run", "--message-format=json"][..],
    ] {
        let out = mitts(&ws, &cache, args);
        assert!(
            out.status.success(),
            "{args:?} failed (status {:?}):\nstdout:\n{}\nstderr:\n{}",
            out.status.code(),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        let recs = parse_ndjson(&out.stdout);
        assert!(
            recs.iter().any(|r| r["reason"] == "compiler-message"
                && r["message"]["rendered"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("unused_in_dep")),
            "{args:?} missing compiler-message JSON for pipelined dep_lib:\n{recs:?}"
        );
        assert!(
            !recs.iter().any(|r| r.get("$message_type").is_some()),
            "{args:?} leaked internal rustc artifact notification:\n{recs:?}"
        );
        assert_eq!(
            recs.last(),
            Some(&serde_json::json!({"reason": "build-finished", "success": true}))
        );
    }

    // 2. `json-diagnostic-short` & `json,json-diagnostic-short`: single-line `rendered` in JSON.
    for fmt in ["json-diagnostic-short", "json,json-diagnostic-short"] {
        let out = mitts(
            &ws,
            &cache,
            &["check", "--workspace", "--message-format", fmt],
        );
        assert!(out.status.success());
        let recs = parse_ndjson(&out.stdout);
        let rendered = recs
            .iter()
            .find(|r| r["reason"] == "compiler-message")
            .and_then(|r| r["message"]["rendered"].as_str())
            .expect("compiler-message rendered");
        assert!(
            rendered.contains("lib.rs:1:") && !rendered.contains("\n  --> "),
            "{fmt} rendered should be single-line short format, got: {rendered:?}"
        );
    }

    // 3. `json-diagnostic-rendered-ansi` & `json,json-diagnostic-rendered-ansi`: ANSI in `rendered`.
    for fmt in [
        "json-diagnostic-rendered-ansi",
        "json,json-diagnostic-rendered-ansi",
    ] {
        let out = mitts(
            &ws,
            &cache,
            &["check", "--workspace", "--message-format", fmt],
        );
        assert!(out.status.success());
        let recs = parse_ndjson(&out.stdout);
        let rendered = recs
            .iter()
            .find(|r| r["reason"] == "compiler-message")
            .and_then(|r| r["message"]["rendered"].as_str())
            .expect("compiler-message rendered");
        assert!(
            rendered.contains("\u{1b}["),
            "{fmt} rendered should contain ANSI escapes, got: {rendered:?}"
        );
    }

    // 4. `json-render-diagnostics` (+ short/ansi modifiers): diagnostics on stderr, ONLY build-finished on stdout.
    for (fmt, expect_short, expect_ansi) in [
        ("json-render-diagnostics", false, false),
        ("json-render-diagnostics,json-diagnostic-short", true, false),
        (
            "json-render-diagnostics,json-diagnostic-rendered-ansi",
            false,
            true,
        ),
    ] {
        let out = mitts(
            &ws,
            &cache,
            &["check", "--workspace", "--message-format", fmt],
        );
        assert!(out.status.success());
        let recs = parse_ndjson(&out.stdout);
        assert_eq!(
            recs,
            vec![serde_json::json!({"reason": "build-finished", "success": true})],
            "{fmt} must omit compiler-message from stdout"
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("unused_in_dep"),
            "{fmt} must render diagnostic to stderr:\n{stderr}"
        );
        if expect_short {
            assert!(
                !stderr.contains("\n  --> "),
                "{fmt} stderr must be short:\n{stderr}"
            );
        }
        if expect_ansi {
            assert!(
                stderr.contains("\u{1b}["),
                "{fmt} stderr must contain ANSI:\n{stderr}"
            );
        }
    }
}

#[test]
fn cross_format_cache_isolation_and_invalid_format_rejection() {
    let tmp = TempDir::new().unwrap();
    let cache = tmp.path().join("cache");
    let proj = tmp.path().join("proj_iso");
    write_crate(&proj, "pub fn iso() { let unused_iso = 1; }\n");

    // Exercise both ActionCache and FastBuildLock (`MITTS_FAST_LOCK=1`) across format switches!
    let fast_env = [("MITTS_FAST_LOCK", "1")];
    let human = mitts_with_env(&proj, &cache, &["check"], &fast_env);
    assert!(human.status.success());

    let short = mitts_with_env(
        &proj,
        &cache,
        &["check", "--message-format=short"],
        &fast_env,
    );
    let short_stderr = String::from_utf8_lossy(&short.stderr);
    assert!(short_stderr.contains("src/lib.rs:1:") && !short_stderr.contains("\n  --> "));

    let json = mitts_with_env(
        &proj,
        &cache,
        &["check", "--message-format=json"],
        &fast_env,
    );
    let json_recs = parse_ndjson(&json.stdout);
    assert!(json_recs.iter().any(|r| r["reason"] == "compiler-message"));

    // Warm fast-lock hit on `short` again must still emit short format, never JSON or human!
    let short_warm = mitts_with_env(
        &proj,
        &cache,
        &["check", "--message-format=short"],
        &fast_env,
    );
    let sw_stderr = String::from_utf8_lossy(&short_warm.stderr);
    assert_eq!(String::from_utf8_lossy(&short_warm.stdout).trim(), "");
    assert!(sw_stderr.contains("src/lib.rs:1:") && !sw_stderr.contains("\"reason\""));

    // Conflicting and unknown `--message-format` combinations exit 2 with empty stdout.
    for bad in [
        "bogus",
        "human,json",
        "short,json",
        "json,json-render-diagnostics",
        "json-diagnostic-short,json-diagnostic-rendered-ansi",
    ] {
        let out = mitts(&proj, &cache, &["check", "--message-format", bad]);
        assert_eq!(
            out.status.code(),
            Some(2),
            "{bad} should fail clap validation"
        );
        assert!(out.stdout.is_empty(), "{bad} must not write to stdout");
    }
}
