/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! `cargo test` builds with the `test` profile (and `bench` under `--release`), which
//! inherits its base profile and overrides the keys it sets. ahash's `[profile.test]
//! opt-level = 2` is what lets its `#[no_panic]` tests link; built with `dev` they fail.

use mitts::adapter::rust::profile::resolve_unit_profile_for;
use std::fs;
use tempfile::TempDir;

#[test]
fn test_and_bench_profiles_inherit_their_base_and_override_it() {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/lib.rs"), "").unwrap();
    fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"prof\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
         [profile.dev]\nopt-level = 1\ndebug = 1\n\n\
         [profile.test]\ninherits = \"dev\"\nopt-level = 2\nlto = \"fat\"\n\n\
         [profile.release]\ncodegen-units = 4\n\n\
         [profile.bench]\ninherits = \"release\"\ncodegen-units = 1\n\n\
         [profile.dev.package.prof]\noverflow-checks = false\n",
    )
    .unwrap();
    let manifest = dir.path().join("Cargo.toml");
    let resolve = |release: bool, test: bool| {
        resolve_unit_profile_for(
            &manifest,
            "prof",
            true,
            false,
            release,
            test,
            Some(dir.path()),
        )
        .unwrap()
    };

    let dev = resolve(false, false);
    assert_eq!(dev.opt_level, "1");
    assert_eq!(dev.debuginfo, 1);
    assert_eq!(dev.lto, None);
    assert!(!dev.overflow_checks, "package override of the base profile");

    let test = resolve(false, true);
    assert_eq!(test.opt_level, "2", "the test profile overrides opt-level");
    assert_eq!(test.debuginfo, 1, "and inherits the rest of dev");
    assert_eq!(test.lto.as_deref(), Some("fat"));
    assert!(
        !test.overflow_checks,
        "package overrides of dev are inherited too"
    );

    let release = resolve(true, false);
    assert_eq!(release.opt_level, "3");
    assert_eq!(release.codegen_units, Some(4));

    let bench = resolve(true, true);
    assert_eq!(bench.opt_level, "3", "bench inherits release");
    assert_eq!(bench.codegen_units, Some(1), "and overrides codegen-units");
}

#[test]
fn test_profile_can_inherit_release_settings() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("Cargo.toml"), "[package]\nname='prof'\nversion='0.1.0'\n[profile.release]\ndebug=1\n[profile.test]\ninherits='release'\n").unwrap();
    let manifest = dir.path().join("Cargo.toml");
    let test = resolve_unit_profile_for(
        &manifest,
        "prof",
        true,
        false,
        false,
        true,
        Some(dir.path()),
    )
    .unwrap();
    assert_eq!(test.opt_level, "3");
    assert_eq!(test.debuginfo, 1);
    assert!(!test.debug_assertions);
}

#[test]
fn test_command_accepts_builtin_inheritance_with_unused_custom_profile() {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    let manifest = "[package]\nname='prof'\nversion='0.1.0'\nedition='2021'\n[profile.test]\ninherits='dev'\ndebug=1\nincremental=true\ncodegen-units=256\n[profile.profiling]\ninherits='release'\ndebug=true\nlto='fat'\n";
    fs::write(dir.path().join("Cargo.toml"), manifest).unwrap();
    fs::write(dir.path().join("src/lib.rs"), "#[test] fn ok() {}\n").unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_mitts"))
        .current_dir(dir.path())
        .args(["test", "--no-run", "--offline", "--cache-dir"])
        .arg(dir.path().join("cache"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("Cargo.toml")).unwrap(),
        manifest
    );
}
