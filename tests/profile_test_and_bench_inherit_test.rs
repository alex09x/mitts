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
         [profile.test]\nopt-level = 2\nlto = \"fat\"\n\n\
         [profile.release]\ncodegen-units = 4\n\n\
         [profile.bench]\ncodegen-units = 1\n\n\
         [profile.dev.package.prof]\noverflow-checks = false\n",
    )
    .unwrap();
    let manifest = dir.path().join("Cargo.toml");
    let resolve = |release: bool, test: bool| {
        resolve_unit_profile_for(&manifest, "prof", true, false, release, test, Some(dir.path()))
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
    assert!(!test.overflow_checks, "package overrides of dev are inherited too");

    let release = resolve(true, false);
    assert_eq!(release.opt_level, "3");
    assert_eq!(release.codegen_units, Some(4));

    let bench = resolve(true, true);
    assert_eq!(bench.opt_level, "3", "bench inherits release");
    assert_eq!(bench.codegen_units, Some(1), "and overrides codegen-units");
}
