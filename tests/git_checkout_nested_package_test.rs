/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Cargo finds a git dependency's package anywhere in the checkout (`ops::read_packages`):
//! zed's livekit-rust-sdks has `webrtc-sys-build` in `webrtc-sys/build`, inside another
//! package's directory. Mitts looked only at the root, one level down and `crates/*`, and
//! failed with "commit … contains no package webrtc-sys-build 0.3.13". Hidden directories
//! and a `target` directory beside a manifest are skipped, as by Cargo.

use mitts::adapter::rust::source_lookup::find_crate_source_dir;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

fn package(dir: &Path, name: &str, version: &str) {
    fs::create_dir_all(dir).unwrap();
    fs::write(
        dir.join("Cargo.toml"),
        format!("[package]\nname = \"{name}\"\nversion = \"{version}\"\n"),
    )
    .unwrap();
}

#[test]
fn a_package_nested_in_another_packages_directory_is_found() {
    let temp = TempDir::new().unwrap();
    let old = (std::env::var_os("HOME"), std::env::var_os("CARGO_HOME"));
    std::env::set_var("HOME", temp.path().join("home"));
    std::env::set_var("CARGO_HOME", temp.path().join("cargo-home"));

    const HASH: &str = "fbda05eb278b4a7c5be8fc9b2a92e9b62e732236";
    let commit = temp
        .path()
        .join("cargo-home/git/checkouts/livekit-rust-sdks-d281d0d44fd23f40/fbda05e");
    fs::create_dir_all(&commit).unwrap();
    fs::write(
        commit.join("Cargo.toml"),
        "[workspace]\nmembers = [\"webrtc-sys\", \"webrtc-sys/build\"]\n",
    )
    .unwrap();
    package(&commit.join("webrtc-sys"), "webrtc-sys", "0.3.13");
    package(
        &commit.join("webrtc-sys/build"),
        "webrtc-sys-build",
        "0.3.13",
    );
    package(&commit.join("webrtc-sys/target/pkg"), "in-target", "1.0.0");
    package(&commit.join(".github/pkg"), "in-hidden", "1.0.0");
    package(&commit.join("tools/deep/er/pkg"), "deep", "2.0.0");

    let src = format!("git+https://github.com/zed-industries/livekit-rust-sdks?rev={HASH}#{HASH}");
    let found = find_crate_source_dir("webrtc-sys-build", "0.3.13", Some(&src));
    let deep = find_crate_source_dir("deep", "2.0.0", Some(&src));
    let in_target = find_crate_source_dir("in-target", "1.0.0", Some(&src));
    let in_hidden = find_crate_source_dir("in-hidden", "1.0.0", Some(&src));

    for (name, value) in [("HOME", old.0), ("CARGO_HOME", old.1)] {
        match value {
            Some(v) => std::env::set_var(name, v),
            None => std::env::remove_var(name),
        }
    }
    assert_eq!(found, Some(commit.join("webrtc-sys/build")));
    assert_eq!(deep, Some(commit.join("tools/deep/er/pkg")));
    assert_eq!(
        in_target, None,
        "a target directory beside a manifest is skipped"
    );
    assert_eq!(in_hidden, None, "hidden directories are skipped");
}
