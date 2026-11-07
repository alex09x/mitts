/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Regression tests for issue #72: `RUSTC_WRAPPER` stays uncached by default with a clear warning,
//! while explicit opt-in via `MITTS_RUSTC_WRAPPER_CACHE=1` caches units and build scripts in a
//! wrapper-scoped namespace (resolved path + executable SHA-256 + optional
//! `MITTS_RUSTC_WRAPPER_SALT`) isolated from unwrapped builds.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;

fn write_file(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

fn write_executable(path: &Path, body: &str) {
    write_file(path, body);
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

struct ReproWorkspace {
    dir: TempDir,
    root: PathBuf,
    cache: PathBuf,
    target: PathBuf,
    wrapper: PathBuf,
    log: PathBuf,
}

impl ReproWorkspace {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("ws");
        let cache = dir.path().join("cache");
        let target = dir.path().join("target");
        let wrapper = dir.path().join("wrap.sh");
        let log = dir.path().join("wrapper.log");

        write_file(
            &root.join("Cargo.toml"),
            "[workspace]\nresolver = \"2\"\nmembers = [\"crates/a\", \"crates/b\"]\n",
        );
        write_file(
            &root.join("crates/a/Cargo.toml"),
            "[package]\nname = \"a\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        );
        write_file(
            &root.join("crates/a/src/lib.rs"),
            "pub fn f() -> u32 { 1 }\n",
        );
        write_file(
            &root.join("crates/b/Cargo.toml"),
            "[package]\nname = \"b\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
             [features]\nextra = []\n\n\
             [dependencies]\na = { path = \"../a\" }\n",
        );
        write_file(
            &root.join("crates/b/src/lib.rs"),
            "pub fn g() -> u32 { a::f() + 1 }\n",
        );
        write_executable(
            &wrapper,
            &format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nexec \"$@\"\n",
                log.display()
            ),
        );

        Self {
            dir,
            root,
            cache,
            target,
            wrapper,
            log,
        }
    }

    fn mitts(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mitts"));
        cmd.current_dir(&self.root)
            .args(args)
            .arg("--offline")
            .arg("--cache-dir")
            .arg(&self.cache)
            .arg("--target-dir")
            .arg(&self.target)
            .env("CARGO_HOME", self.dir.path().join("cargo-home"))
            .env_remove("RUSTC_WRAPPER")
            .env_remove("CARGO_BUILD_RUSTC_WRAPPER")
            .env_remove("RUSTC_WORKSPACE_WRAPPER")
            .env_remove("CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER")
            .env_remove("MITTS_RUSTC_WRAPPER_CACHE")
            .env_remove("MITTS_RUSTC_WRAPPER_SALT")
            .env_remove("MITTS_FAST_LOCK");
        for (k, v) in env {
            cmd.env(k, v);
        }
        cmd.output().unwrap()
    }

    fn explain_keys(&self, out_file: &Path, env: &[(&str, &str)]) -> String {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mitts"));
        cmd.current_dir(&self.root)
            .args(["explain", "--keys"])
            .arg(out_file)
            .arg("--cache-dir")
            .arg(&self.cache)
            .env("CARGO_HOME", self.dir.path().join("cargo-home"))
            .env_remove("RUSTC_WRAPPER")
            .env_remove("CARGO_BUILD_RUSTC_WRAPPER")
            .env_remove("RUSTC_WORKSPACE_WRAPPER")
            .env_remove("CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER")
            .env_remove("MITTS_RUSTC_WRAPPER_CACHE")
            .env_remove("MITTS_RUSTC_WRAPPER_SALT");
        for (k, v) in env {
            cmd.env(k, v);
        }
        let out = cmd.output().unwrap();
        assert_ok(&out);
        fs::read_to_string(out_file).unwrap()
    }

    fn take_logged_lines(&self) -> Vec<String> {
        let content = fs::read_to_string(&self.log).unwrap_or_default();
        let _ = fs::remove_file(&self.log);
        content
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(str::to_string)
            .collect()
    }
}

fn stderr_str(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn stdout_str(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn assert_ok(out: &Output) {
    assert!(
        out.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        stdout_str(out),
        stderr_str(out)
    );
}

#[test]
fn issue_72_default_without_opt_in_stays_uncached_and_warns() {
    let ws = ReproWorkspace::new();
    let wrap = ws.wrapper.to_str().unwrap();

    for run in 1..=2 {
        let out = ws.mitts(&["check", "--workspace"], &[("RUSTC_WRAPPER", wrap)]);
        assert_ok(&out);
        let err = stderr_str(&out);
        assert!(
            err.contains("[mitts::warn] a rustc wrapper is configured"),
            "{err}"
        );
        assert!(err.contains("MITTS_RUSTC_WRAPPER_CACHE=1"), "{err}");
        assert!(
            err.contains("compiled=2 cached=0 tainted=2"),
            "run {run}: {err}"
        );
        assert_eq!(ws.take_logged_lines().len(), 2, "run {run}");
    }

    let shared_objects = fs::read_dir(ws.cache.join("cas/objects"))
        .map(|entries| entries.flatten().count())
        .unwrap_or(0);
    assert_eq!(shared_objects, 0);
}

#[test]
fn issue_72_opt_in_reuses_unchanged_crates_across_runs_and_leaf_feature_switches() {
    let ws = ReproWorkspace::new();
    let wrap = ws.wrapper.to_str().unwrap();
    let env = [("RUSTC_WRAPPER", wrap), ("MITTS_RUSTC_WRAPPER_CACHE", "1")];

    let run1 = ws.mitts(&["check", "--workspace"], &env);
    assert_ok(&run1);
    let err1 = stderr_str(&run1);
    assert!(
        !err1.contains("[mitts::warn] a rustc wrapper is configured"),
        "{err1}"
    );
    assert!(err1.contains("compiled=2 cached=0 tainted=0"), "{err1}");
    assert_eq!(ws.take_logged_lines().len(), 2);

    let run2 = ws.mitts(&["check", "--workspace"], &env);
    assert_ok(&run2);
    assert!(
        stderr_str(&run2).contains("compiled=0 cached=2"),
        "{}",
        stderr_str(&run2)
    );
    assert!(ws.take_logged_lines().is_empty());

    let run3 = ws.mitts(&["check", "--workspace", "--features", "b/extra"], &env);
    assert_ok(&run3);
    assert!(
        stderr_str(&run3).contains("compiled=1 cached=1"),
        "{}",
        stderr_str(&run3)
    );
    let lines3 = ws.take_logged_lines();
    assert_eq!(lines3.len(), 1, "{lines3:?}");
    assert!(lines3[0].contains("--crate-name b"), "{lines3:?}");
}

#[test]
fn issue_72_namespace_isolation_path_lookup_and_wrapper_or_salt_invalidation() {
    let ws = ReproWorkspace::new();
    write_file(
        &ws.root.join("crates/a/build.rs"),
        "fn main() { println!(\"cargo:rustc-cfg=from_build_rs\"); }\n",
    );
    write_file(
        &ws.root.join("crates/a/src/lib.rs"),
        "#[cfg(wrapped_v2)] pub fn tag() -> &'static str { \"wrapped-v2\" }\n\
         #[cfg(all(wrapped_v1, not(wrapped_v2)))] pub fn tag() -> &'static str { \"wrapped-v1\" }\n\
         #[cfg(not(any(wrapped_v1, wrapped_v2)))] pub fn tag() -> &'static str { \"unwrapped\" }\n",
    );
    write_file(
        &ws.root.join("crates/b/src/lib.rs"),
        "pub fn msg() -> &'static str { a::tag() }\n",
    );
    write_file(
        &ws.root.join("crates/b/src/main.rs"),
        "fn main() { println!(\"{}\", b::msg()); }\n",
    );

    write_executable(
        &ws.wrapper,
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\ncompiler=$1\nshift\nexec \"$compiler\" \"$@\" --cfg wrapped_v1\n",
            ws.log.display()
        ),
    );
    write_file(
        &ws.root.join(".cargo/config.toml"),
        &format!("[build]\nrustc-wrapper = \"{}\"\n", ws.wrapper.display()),
    );

    let bin = ws.target.join("debug/b");
    let run_bin = |bin_path: &Path| -> String {
        let out = Command::new(bin_path).output().unwrap();
        assert!(out.status.success());
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    };

    // 1. Populate wrapper-scoped cache (`MITTS_RUSTC_WRAPPER_CACHE=1` + `MITTS_FAST_LOCK=1`).
    let wrapped1 = ws.mitts(
        &["build", "-p", "b"],
        &[("MITTS_RUSTC_WRAPPER_CACHE", "1"), ("MITTS_FAST_LOCK", "1")],
    );
    assert_ok(&wrapped1);
    assert_eq!(run_bin(&bin), "wrapped-v1");
    assert!(!ws.take_logged_lines().is_empty());

    // 2. Unwrapped build (`RUSTC_WRAPPER=""` clears config wrapper) MUST NOT reuse wrapped entries.
    let unwrapped = ws.mitts(
        &["build", "-p", "b"],
        &[("RUSTC_WRAPPER", ""), ("MITTS_FAST_LOCK", "1")],
    );
    assert_ok(&unwrapped);
    assert!(
        stderr_str(&unwrapped).contains("cached=0"),
        "{}",
        stderr_str(&unwrapped)
    );
    assert_eq!(run_bin(&bin), "unwrapped");

    // 3. Reverse direction + PATH-resolved bare wrapper + `explain --keys` parity.
    let ws2 = ReproWorkspace::new();
    let bin_dir = ws2.dir.path().join("bin");
    let bare_wrap = bin_dir.join("my-mitts-wrap");
    write_executable(
        &bare_wrap,
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nexec \"$@\"\n",
            ws2.log.display()
        ),
    );
    let path_with_bin = format!(
        "{}:{}",
        bin_dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let keys_unwrapped = ws2.explain_keys(&ws2.dir.path().join("keys1.json"), &[]);
    let keys_wrapped = ws2.explain_keys(
        &ws2.dir.path().join("keys2.json"),
        &[
            ("PATH", &path_with_bin),
            ("RUSTC_WRAPPER", "my-mitts-wrap"),
            ("MITTS_RUSTC_WRAPPER_CACHE", "1"),
        ],
    );
    assert_ne!(
        keys_unwrapped, keys_wrapped,
        "opt-in wrapper keys must differ from unwrapped keys"
    );

    let warm_unwrapped = ws2.mitts(&["check", "--workspace"], &[]);
    assert_ok(&warm_unwrapped);
    let first_wrapped = ws2.mitts(
        &["check", "--workspace"],
        &[
            ("PATH", &path_with_bin),
            ("RUSTC_WRAPPER", "my-mitts-wrap"),
            ("MITTS_RUSTC_WRAPPER_CACHE", "1"),
        ],
    );
    assert_ok(&first_wrapped);
    assert!(
        stderr_str(&first_wrapped).contains("compiled=2 cached=0"),
        "{}",
        stderr_str(&first_wrapped)
    );

    // 4. Editing wrapper script bytes at the SAME path invalidates FastBuildLock + ActionKeys.
    write_executable(
        &ws.wrapper,
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\ncompiler=$1\nshift\nexec \"$compiler\" \"$@\" --cfg wrapped_v2\n",
            ws.log.display()
        ),
    );
    let wrapped_v2 = ws.mitts(
        &["build", "-p", "b"],
        &[("MITTS_RUSTC_WRAPPER_CACHE", "1"), ("MITTS_FAST_LOCK", "1")],
    );
    assert_ok(&wrapped_v2);
    assert!(
        stderr_str(&wrapped_v2).contains("cached=0"),
        "{}",
        stderr_str(&wrapped_v2)
    );
    assert_eq!(run_bin(&bin), "wrapped-v2");

    // 5. Changing `MITTS_RUSTC_WRAPPER_SALT` invalidates wrapper-scoped cache, then repeats hit.
    let salted1 = ws2.mitts(
        &["check", "--workspace"],
        &[
            ("PATH", &path_with_bin),
            ("RUSTC_WRAPPER", "my-mitts-wrap"),
            ("MITTS_RUSTC_WRAPPER_CACHE", "1"),
            ("MITTS_RUSTC_WRAPPER_SALT", "schema-v2"),
        ],
    );
    assert_ok(&salted1);
    assert!(
        stderr_str(&salted1).contains("compiled=2 cached=0"),
        "{}",
        stderr_str(&salted1)
    );
    let salted2 = ws2.mitts(
        &["check", "--workspace"],
        &[
            ("PATH", &path_with_bin),
            ("RUSTC_WRAPPER", "my-mitts-wrap"),
            ("MITTS_RUSTC_WRAPPER_CACHE", "1"),
            ("MITTS_RUSTC_WRAPPER_SALT", "schema-v2"),
        ],
    );
    assert_ok(&salted2);
    assert!(
        stderr_str(&salted2).contains("compiled=0 cached=2"),
        "{}",
        stderr_str(&salted2)
    );

    // 6. Missing wrapper under `MITTS_RUSTC_WRAPPER_CACHE=1` fails closed explicitly.
    let missing = ws2.mitts(
        &["check", "--workspace"],
        &[
            (
                "RUSTC_WRAPPER",
                ws2.dir.path().join("missing-wrap.sh").to_str().unwrap(),
            ),
            ("MITTS_RUSTC_WRAPPER_CACHE", "1"),
        ],
    );
    assert!(!missing.status.success());
    assert!(
        stderr_str(&missing).contains("MITTS_RUSTC_WRAPPER_CACHE=1"),
        "{}",
        stderr_str(&missing)
    );
}
