/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! `--config KEY=VALUE` and `--config FILE` (issue #109): the highest-precedence Cargo
//! configuration layer.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use tempfile::TempDir;

fn write_file(path: impl AsRef<Path>, contents: &str) {
    let path = path.as_ref();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// A binary reporting a `--cfg from_config` and a compile-time environment variable.
fn project() -> TempDir {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    write_file(
        root.join("Cargo.toml"),
        "[package]\nname = \"c\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\
         [lints.rust]\nunexpected_cfgs = \"allow\"\n",
    );
    write_file(
        root.join("src/main.rs"),
        "fn main() {\n\
         println!(\"cfg={} greeting={}\", cfg!(from_config), \
         option_env!(\"GREETING\").unwrap_or(\"-\"));\n}\n",
    );
    temp
}

fn mitts(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mitts"))
        .args(args)
        .arg("--cache-dir")
        .arg(root.join("mitts-cache"))
        .current_dir(root)
        .env_remove("CARGO_TARGET_DIR")
        .env_remove("CARGO_BUILD_TARGET_DIR")
        .env_remove("RUSTFLAGS")
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .env_remove("CARGO_BUILD_RUSTFLAGS")
        .env_remove("GREETING")
        .output()
        .expect("run Mitts")
}

fn assert_ok(output: &Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn host() -> String {
    let output = Command::new("rustc").arg("-vV").output().unwrap();
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .find_map(|l| l.strip_prefix("host: ").map(str::to_string))
        .unwrap()
}

/// `--cfg from_config` as target rustflags: they add to a machine's own target rustflags,
/// while `build.rustflags` would be shadowed by them (Cargo's precedence).
fn cfg_flag() -> String {
    format!("target.{}.rustflags=[\"--cfg\", \"from_config\"]", host())
}

fn run_binary(path: &Path) -> String {
    let output = Command::new(path).output().expect("run the binary");
    assert!(output.status.success());
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn inline_values_reach_the_build_and_go_away_without_the_flag() {
    let temp = project();
    let root = temp.path();
    assert_ok(&mitts(
        root,
        &[
            "build",
            "-q",
            "--config",
            &cfg_flag(),
            "--config",
            "env.GREETING=\"hi\"",
        ],
    ));
    assert_eq!(
        run_binary(&root.join("target/debug/c")),
        "cfg=true greeting=hi\n"
    );

    // The next build without --config is the plain build again.
    assert_ok(&mitts(root, &["build", "-q"]));
    assert_eq!(
        run_binary(&root.join("target/debug/c")),
        "cfg=false greeting=-\n"
    );
}

#[test]
fn a_config_file_and_a_relative_target_dir_from_the_current_directory() {
    let temp = project();
    let root = temp.path();
    write_file(
        root.join("extra.toml"),
        &format!(
            "[target.{}]\nrustflags = [\"--cfg\", \"from_config\"]\n",
            host()
        ),
    );
    assert_ok(&mitts(
        root,
        &[
            "build",
            "-q",
            "--config",
            "extra.toml",
            "--config",
            "build.target-dir=\"cfg-out\"",
        ],
    ));
    assert_eq!(
        run_binary(&root.join("cfg-out/debug/c")),
        "cfg=true greeting=-\n"
    );
    assert!(!root.join("target/debug/c").exists());
}

#[test]
fn the_flag_overrides_project_configuration() {
    let temp = project();
    let root = temp.path();
    write_file(
        root.join(".cargo/config.toml"),
        "[env]\nGREETING = \"from-project\"\n",
    );
    assert_ok(&mitts(
        root,
        &["build", "-q", "--config", "env.GREETING=\"from-cli\""],
    ));
    assert_eq!(
        run_binary(&root.join("target/debug/c")),
        "cfg=false greeting=from-cli\n"
    );
}

#[test]
fn invalid_values_are_errors() {
    let temp = project();
    let root = temp.path();
    for value in ["not toml =", "missing.toml", "a.b=1"] {
        let args: Vec<&str> = if value == "a.b=1" {
            vec!["build", "--config", "a.b=1", "--config", "a.b=2"]
        } else {
            vec!["build", "--config", value]
        };
        let output = mitts(root, &args);
        assert!(!output.status.success(), "{args:?} succeeded");
        assert!(String::from_utf8_lossy(&output.stderr).contains("--config"));
    }
}
