/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! `--timings[=html,json]` and `-Z` (issue #109).

use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use tempfile::TempDir;

fn write_file(path: impl AsRef<Path>, contents: &str) {
    let path = path.as_ref();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// A binary with a library dependency: two units.
fn project() -> TempDir {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    write_file(
        root.join("Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\
         [dependencies]\nhelper = { path = \"helper\" }\n",
    );
    write_file(root.join("src/main.rs"), "fn main() { helper::h(); }\n");
    write_file(
        root.join("helper/Cargo.toml"),
        "[package]\nname = \"helper\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write_file(root.join("helper/src/lib.rs"), "pub fn h() {}\n");
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

#[test]
fn timings_writes_an_html_report_with_every_unit() {
    let temp = project();
    let root = temp.path();
    let output = mitts(root, &["build", "-q", "--timings"]);
    assert_ok(&output);
    assert!(String::from_utf8_lossy(&output.stderr).contains("Timing report saved to"));
    let html = fs::read_to_string(root.join("target/cargo-timings/cargo-timing.html")).unwrap();
    assert!(html.contains("Mitts build timings"));
    assert!(html.contains("helper helper (lib)"), "{html}");
    assert!(html.contains("app app (bin)"), "{html}");
    let stamped = fs::read_dir(root.join("target/cargo-timings"))
        .unwrap()
        .flatten()
        .filter(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            name.starts_with("cargo-timing-") && name.ends_with(".html")
        })
        .count();
    assert_eq!(stamped, 1);
}

#[test]
fn timings_json_lists_units_with_their_outcome() {
    let temp = project();
    let root = temp.path();
    assert_ok(&mitts(root, &["build", "-q", "--timings=json"]));
    let json: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(root.join("target/cargo-timings/cargo-timing.json")).unwrap(),
    )
    .unwrap();
    let units = json["units"].as_array().unwrap();
    assert_eq!(units.len(), 2, "{json}");
    assert!(units.iter().all(|u| u["outcome"] == "compiled"), "{json}");
    assert!(!root.join("target/cargo-timings/cargo-timing.html").exists());
    let unknown = mitts(root, &["build", "--timings=xml"]);
    assert!(!unknown.status.success());
}

/// Where the fast build lock would answer, `--timings` still reports the units of a real
/// run instead of overwriting the report with an empty one (review of #116).
#[test]
fn timings_bypasses_the_fast_lock() {
    let temp = project();
    let root = temp.path();
    let build = |extra: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_mitts"))
            .arg("build")
            .args(extra)
            .arg("--cache-dir")
            .arg(root.join("mitts-cache"))
            .current_dir(root)
            .env("MITTS_FAST_LOCK", "1")
            .env("RUSTC_WRAPPER", "")
            .env_remove("CARGO_TARGET_DIR")
            .env_remove("CARGO_BUILD_TARGET_DIR")
            .output()
            .unwrap()
    };
    assert_ok(&build(&["-q"]));
    assert_ok(&build(&["-q"]));
    assert_ok(&build(&["-q", "--timings=json"]));
    let json: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(root.join("target/cargo-timings/cargo-timing.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(json["units"].as_array().unwrap().len(), 2, "{json}");
}

#[test]
fn unstable_flags_are_accepted_or_named() {
    let temp = project();
    let root = temp.path();
    assert_ok(&mitts(root, &["-Zunstable-options", "build", "-q"]));
    assert_ok(&mitts(root, &["build", "-q", "-Z", "unstable-options"]));
    let other = mitts(root, &["build", "-Z", "build-std"]);
    assert!(!other.status.success());
    let stderr = String::from_utf8_lossy(&other.stderr);
    assert!(
        stderr.contains("-Z build-std") && stderr.contains("RUSTFLAGS"),
        "{stderr}"
    );
}
