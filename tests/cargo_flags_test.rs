/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Cargo build flags that `mitts build` used to reject (issue #109): `--workspace`,
//! `--tests`, `--test`, `--all-targets`, `--color`, and `build.target-dir` from
//! `.cargo/config.toml` / `CARGO_BUILD_TARGET_DIR`.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use tempfile::TempDir;

fn write_file(path: impl AsRef<Path>, contents: &str) {
    let path = path.as_ref();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// Members `a` (lib, bin `tool`, unit test, integration test `it`, example `ex`,
/// bench `bn`) and `b` (lib).
fn workspace() -> TempDir {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    write_file(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"a\", \"b\"]\nresolver = \"2\"\n",
    );
    write_file(
        root.join("a/Cargo.toml"),
        "[package]\nname = \"a\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\
         [[bench]]\nname = \"bn\"\nharness = false\n",
    );
    write_file(
        root.join("a/src/lib.rs"),
        "pub fn a() {}\n#[cfg(test)]\nmod t {\n    #[test]\n    fn unit() {}\n}\n",
    );
    write_file(root.join("a/src/bin/tool.rs"), "fn main() {}\n");
    write_file(root.join("a/tests/it.rs"), "#[test]\nfn it() {}\n");
    write_file(root.join("a/examples/ex.rs"), "fn main() {}\n");
    write_file(root.join("a/benches/bn.rs"), "fn main() {}\n");
    write_file(
        root.join("b/Cargo.toml"),
        "[package]\nname = \"b\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write_file(root.join("b/src/lib.rs"), "pub fn b() {}\n");
    temp
}

fn mitts(root: &Path, args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mitts"));
    cmd.args(args)
        .arg("--cache-dir")
        .arg(root.join("mitts-cache"))
        .current_dir(root)
        .env_remove("CARGO_TARGET_DIR")
        .env_remove("CARGO_BUILD_TARGET_DIR")
        .env_remove("CARGO_TERM_COLOR")
        .env_remove("CLICOLOR_FORCE")
        .env_remove("NO_COLOR");
    for (key, value) in env {
        cmd.env(key, value);
    }
    cmd.output().expect("run Mitts")
}

fn assert_ok(output: &Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn deps(root: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(root.join("target/debug/deps"))
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

#[test]
fn workspace_builds_every_member() {
    let temp = workspace();
    let root = temp.path();
    assert_ok(&mitts(
        &root.join("a"),
        &["build", "-q", "--workspace"],
        &[],
    ));
    assert!(root.join("target/debug/liba.rlib").is_file());
    assert!(root.join("target/debug/libb.rlib").is_file());

    // `--all` is the old spelling.
    let temp = workspace();
    let root = temp.path();
    assert_ok(&mitts(root, &["build", "-q", "--all"], &[]));
    assert!(root.join("target/debug/libb.rlib").is_file());
}

#[test]
fn tests_and_test_build_test_targets_without_running_them() {
    let temp = workspace();
    let root = temp.path();
    assert_ok(&mitts(root, &["build", "-q", "-p", "a", "--tests"], &[]));
    let built = deps(root);
    assert!(built.iter().any(|n| n == "a-it"), "{built:?}");
    assert!(
        built.iter().any(|n| n.starts_with("a-a_unit_test")),
        "{built:?}"
    );

    let temp = workspace();
    let root = temp.path();
    assert_ok(&mitts(
        root,
        &["build", "-q", "-p", "a", "--test", "it"],
        &[],
    ));
    let built = deps(root);
    assert!(built.iter().any(|n| n == "a-it"), "{built:?}");
    assert!(
        !built.iter().any(|n| n.starts_with("a-a_unit_test")),
        "{built:?}"
    );
}

#[test]
fn all_targets_builds_bins_tests_examples_and_benches() {
    let temp = workspace();
    let root = temp.path();
    assert_ok(&mitts(
        root,
        &["build", "-q", "-p", "a", "--all-targets"],
        &[],
    ));
    assert!(root.join("target/debug/tool").is_file());
    assert!(root.join("target/debug/examples/ex").is_file());
    let built = deps(root);
    assert!(built.iter().any(|n| n == "a-it"), "{built:?}");
    assert!(built.iter().any(|n| n == "a-bn"), "{built:?}");
}

#[test]
fn run_rejects_workspace_and_test_selections() {
    let temp = workspace();
    let root = temp.path();
    for flag in ["--workspace", "--tests", "--all-targets"] {
        let output = mitts(root, &["run", "-p", "a", flag], &[]);
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains(flag));
    }
}

#[test]
fn target_dir_comes_from_cargo_config_and_cargo_build_target_dir() {
    let temp = workspace();
    let root = temp.path();
    write_file(
        root.join(".cargo/config.toml"),
        "[build]\ntarget-dir = \"from-config\"\n",
    );
    assert_ok(&mitts(&root.join("a"), &["build", "-q", "-p", "b"], &[]));
    assert!(root.join("from-config/debug/libb.rlib").is_file());
    assert!(!root.join("target").exists());

    let env_dir = root.join("from-env");
    assert_ok(&mitts(
        root,
        &["build", "-q", "-p", "b"],
        &[("CARGO_BUILD_TARGET_DIR", env_dir.to_str().unwrap())],
    ));
    assert!(env_dir.join("debug/libb.rlib").is_file());
}

/// Output layout and coloring variables are not compilation inputs: setting them must not
/// recompile anything (review of #111).
#[test]
fn target_dir_and_color_variables_do_not_invalidate_the_cache() {
    let temp = workspace();
    let root = temp.path();
    assert_ok(&mitts(root, &["build", "-p", "b"], &[]));
    let target = root.join("target");
    let again = mitts(
        root,
        &["build", "-p", "b"],
        &[
            ("CARGO_TERM_COLOR", "never"),
            ("CARGO_BUILD_TARGET_DIR", target.to_str().unwrap()),
        ],
    );
    assert_ok(&again);
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&again.stdout),
        String::from_utf8_lossy(&again.stderr)
    );
    assert!(text.contains("compiled=0"), "{text}");
}

#[test]
fn color_never_and_always_control_mitts_output() {
    let temp = workspace();
    let root = temp.path();
    let forced = [("CLICOLOR_FORCE", "1")];
    let never = mitts(root, &["build", "--color", "never", "-p", "b"], &forced);
    assert_ok(&never);
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&never.stdout),
        String::from_utf8_lossy(&never.stderr)
    );
    assert!(!text.contains('\u{1b}'), "{text:?}");

    let always = mitts(root, &["build", "--color", "always", "-p", "b"], &[]);
    assert_ok(&always);
    assert!(String::from_utf8_lossy(&always.stdout).contains('\u{1b}'));

    // Cargo's variable applies without the flag; piped output is plain by default.
    let by_env = mitts(
        root,
        &["build", "-p", "b"],
        &[("CARGO_TERM_COLOR", "always")],
    );
    assert!(String::from_utf8_lossy(&by_env.stdout).contains('\u{1b}'));
    let piped = mitts(root, &["build", "-p", "b"], &[]);
    assert!(!String::from_utf8_lossy(&piped.stdout).contains('\u{1b}'));
}
