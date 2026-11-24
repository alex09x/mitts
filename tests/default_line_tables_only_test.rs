/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Mitts' one deliberate departure from Cargo's profile defaults: `dev` and `test` without a
//! `debug` setting get `-C debuginfo=line-tables-only` instead of full debug info. A panic
//! backtrace still names the file and line; any explicit `debug` (Cargo.toml, Cargo config,
//! `CARGO_PROFILE_<NAME>_DEBUG`) is passed as written.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use tempfile::TempDir;

fn write_file(path: impl AsRef<Path>, contents: &str) {
    let path = path.as_ref();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// A library with a dependency and a test that panics on a known line.
fn project(root: &Path, profile: &str) {
    write_file(
        root.join("Cargo.toml"),
        &format!(
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\
             [dependencies]\nhelper = {{ path = \"helper\" }}\n{profile}"
        ),
    );
    write_file(
        root.join("src/lib.rs"),
        "pub fn answer() -> u32 { helper::answer() }\n\n\
         #[cfg(test)]\nmod tests {\n    #[test]\n    fn fails() {\n        \
         assert_eq!(super::answer(), 41);\n    }\n}\n",
    );
    write_file(
        root.join("helper/Cargo.toml"),
        "[package]\nname = \"helper\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write_file(
        root.join("helper/src/lib.rs"),
        "pub fn answer() -> u32 { 42 }\n",
    );
}

fn mitts(root: &Path, args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mitts"));
    cmd.args(args)
        .arg("--cache-dir")
        .arg(root.join("cache"))
        .arg("--target-dir")
        .arg(root.join("target"))
        .current_dir(root)
        .env("CARGO_HOME", root.join("cargo-home"))
        .env_remove("MITTS_CACHE_DIR");
    for name in [
        "CARGO_TARGET_DIR",
        "CARGO_BUILD_TARGET_DIR",
        "CARGO_BUILD_TARGET",
        "CARGO_ENCODED_RUSTFLAGS",
        "RUSTFLAGS",
        "CARGO_BUILD_RUSTFLAGS",
        "RUSTC_WRAPPER",
        "RUSTC_WORKSPACE_WRAPPER",
        "CARGO_BUILD_RUSTC_WRAPPER",
        "CARGO_PROFILE_DEV_DEBUG",
        "CARGO_PROFILE_TEST_DEBUG",
        "RUST_BACKTRACE",
    ] {
        cmd.env_remove(name);
    }
    cmd.envs(env.iter().copied());
    cmd.output().expect("run Mitts")
}

/// The `-C debuginfo=` values rustc got for crate `name`, from a `-vv` log.
fn debuginfo_of(log: &str, name: &str) -> Vec<String> {
    log.lines()
        .filter(|l| l.contains(&format!("--crate-name {name} ")))
        .filter_map(|l| {
            l.split_whitespace()
                .find_map(|w| w.strip_prefix("debuginfo=").map(str::to_string))
        })
        .collect()
}

#[test]
fn dev_and_test_default_to_line_tables_and_backtraces_keep_lines() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("proj");
    project(&root, "");

    let build = mitts(&root, &["build", "-vv"], &[]);
    let log = String::from_utf8_lossy(&build.stderr).to_string();
    assert!(build.status.success(), "{log}");
    assert_eq!(debuginfo_of(&log, "app"), ["line-tables-only"], "{log}");
    assert_eq!(debuginfo_of(&log, "helper"), ["line-tables-only"], "{log}");

    let test = mitts(&root, &["test", "-vv"], &[("RUST_BACKTRACE", "1")]);
    let err = String::from_utf8_lossy(&test.stderr).to_string();
    let out = String::from_utf8_lossy(&test.stdout).to_string();
    assert!(
        !test.status.success(),
        "the test fails on purpose: {out}{err}"
    );
    assert!(
        debuginfo_of(&err, "app")
            .iter()
            .all(|v| v == "line-tables-only"),
        "{err}"
    );
    // The panic message carries its location without debug info; a backtrace frame
    // (`at <path>:<line>:<col>`) needs the line tables. Not on macOS yet: Apple's linker
    // leaves the crate's own objects out of the debug map (empty compile directory after
    // the workspace remap), in every release so far; KYB task-mitts-macos-workspace-lib-
    // debug-map. The fix tried in PR 149 made executables checkout-specific and was
    // deferred by the owner.
    if !cfg!(target_os = "macos") {
        assert!(
            // `src\lib.rs` on Windows, where the MSVC symbolizer gives the line without a
            // column.
            out.lines().any(|l| {
                let l = l.replace('\\', "/");
                l.trim_start().starts_with("at ")
                    && (l.contains("src/lib.rs:7:")
                        || (cfg!(target_env = "msvc") && l.trim_end().ends_with("src/lib.rs:7")))
            }),
            "a backtrace frame names the failing line: {out}"
        );
    }
}

#[test]
fn an_explicit_debug_setting_is_passed_as_written() {
    for (profile, env, expected) in [
        ("[profile.dev]\ndebug = true\n", vec![], "2"),
        ("[profile.dev]\ndebug = \"limited\"\n", vec![], "1"),
        (
            "[profile.dev]\ndebug = \"line-tables-only\"\n",
            vec![],
            "line-tables-only",
        ),
        ("[profile.dev]\ndebug = false\n", vec![], "0"),
        ("", vec![("CARGO_PROFILE_DEV_DEBUG", "full")], "2"),
    ] {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("proj");
        project(&root, profile);
        let out = mitts(&root, &["build", "-vv"], &env);
        let log = String::from_utf8_lossy(&out.stderr).to_string();
        assert!(out.status.success(), "{profile}{env:?}: {log}");
        assert_eq!(
            debuginfo_of(&log, "app"),
            [expected],
            "{profile}{env:?}: {log}"
        );
    }
}

/// A Cargo config profile turns full debug info back on for every project under it, and
/// changes the key: the line-tables build is not reused for it.
#[test]
fn a_cargo_config_profile_turns_full_debug_info_back_on() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("proj");
    project(&root, "");
    let first = mitts(&root, &["build"], &[]);
    assert!(first.status.success());

    write_file(
        root.join(".cargo/config.toml"),
        "[profile.dev]\ndebug = true\n",
    );
    let out = mitts(&root, &["build", "-vv"], &[]);
    let log = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(out.status.success(), "{log}");
    assert_eq!(debuginfo_of(&log, "app"), ["2"], "rebuilt with full: {log}");
    assert_eq!(debuginfo_of(&log, "helper"), ["2"], "{log}");
}
