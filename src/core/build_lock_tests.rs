/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::*;

#[test]
fn test_fast_build_lock_roundtrip() {
    let temp = tempfile::tempdir().unwrap();
    let pdir = temp.path();

    let cargo_toml = pdir.join("Cargo.toml");
    fs::write(
        &cargo_toml,
        "[package]\nname = \"foo\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    let src_dir = pdir.join("src");
    fs::create_dir_all(&src_dir).unwrap();
    let main_rs = src_dir.join("main.rs");
    fs::write(&main_rs, "fn main() {}\n").unwrap();

    let bin = pdir.join("target").join("debug").join("foo");
    fs::create_dir_all(bin.parent().unwrap()).unwrap();
    fs::write(&bin, "binary-content").unwrap();

    let outputs = vec![bin.clone()];
    FastBuildLock::record_state(&RecordStateArgs {
        project_dir: pdir,
        profile: "dev",
        package: &None,
        features: &[],
        no_default_features: false,
        all_features: false,
        toolchain_version: "rustc 1.85.0",
        action_count: 1,
        root_binary: Some(bin.clone()),
        outputs: &outputs,
        stderr: None,
    })
    .unwrap();

    let res = FastBuildLock::check_freshness(&FreshnessCheckArgs {
        project_dir: pdir,
        profile: "dev",
        package: &None,
        features: &[],
        no_default_features: false,
        all_features: false,
        toolchain_version: "rustc 1.85.0",
    });
    match res {
        FreshnessResult::Fresh {
            action_count,
            root_binary,
            ..
        } => {
            assert_eq!(action_count, 1);
            assert_eq!(root_binary, Some(bin));
        }
        FreshnessResult::Stale(r) => panic!("Expected fresh lock, got: {}", r),
    }

    // Modifying source file triggers stale
    fs::write(&main_rs, "fn main() { println!(); }\n").unwrap();
    let res2 = FastBuildLock::check_freshness(&FreshnessCheckArgs {
        project_dir: pdir,
        profile: "dev",
        package: &None,
        features: &[],
        no_default_features: false,
        all_features: false,
        toolchain_version: "rustc 1.85.0",
    });
    assert!(matches!(res2, FreshnessResult::Stale(_)));
}

#[test]
fn test_fast_build_lock_added_file() {
    let temp = tempfile::tempdir().unwrap();
    let pdir = temp.path();

    let cargo_toml = pdir.join("Cargo.toml");
    fs::write(
        &cargo_toml,
        "[package]\nname = \"foo\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    let src_dir = pdir.join("src");
    fs::create_dir_all(&src_dir).unwrap();
    let main_rs = src_dir.join("main.rs");
    fs::write(&main_rs, "fn main() {}\n").unwrap();

    let bin = pdir.join("target").join("debug").join("foo");
    fs::create_dir_all(bin.parent().unwrap()).unwrap();
    fs::write(&bin, "binary-content").unwrap();

    let outputs = vec![bin.clone()];
    FastBuildLock::record_state(&RecordStateArgs {
        project_dir: pdir,
        profile: "dev",
        package: &None,
        features: &[],
        no_default_features: false,
        all_features: false,
        toolchain_version: "rustc 1.85.0",
        action_count: 1,
        root_binary: Some(bin.clone()),
        outputs: &outputs,
        stderr: None,
    })
    .unwrap();

    // Verify it is initially fresh
    let check_args = FreshnessCheckArgs {
        project_dir: pdir,
        profile: "dev",
        package: &None,
        features: &[],
        no_default_features: false,
        all_features: false,
        toolchain_version: "rustc 1.85.0",
    };
    assert!(matches!(
        FastBuildLock::check_freshness(&check_args),
        FreshnessResult::Fresh { .. }
    ));

    // Adding a new file in src/ must trigger stale via directory mtime detection
    std::thread::sleep(std::time::Duration::from_millis(15));
    let probe = src_dir.join("probe.rs");
    fs::write(&probe, "pub fn probe() {}\n").unwrap();

    let res = FastBuildLock::check_freshness(&check_args);
    assert!(
        matches!(res, FreshnessResult::Stale(_)),
        "Expected Stale after adding file, got: {:?}",
        res
    );
}

#[test]
fn test_fast_build_lock_multi_profile() {
    let temp = tempfile::tempdir().unwrap();
    let pdir = temp.path();
    let cargo_toml = pdir.join("Cargo.toml");
    fs::write(
        &cargo_toml,
        "[package]\nname = \"foo\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    let src_dir = pdir.join("src");
    fs::create_dir_all(&src_dir).unwrap();
    fs::write(src_dir.join("main.rs"), "fn main() {}\n").unwrap();

    let bin_dev = pdir.join("target/debug/foo");
    let bin_rel = pdir.join("target/release/foo");
    fs::create_dir_all(bin_dev.parent().unwrap()).unwrap();
    fs::create_dir_all(bin_rel.parent().unwrap()).unwrap();
    fs::write(&bin_dev, "dev-bin").unwrap();
    fs::write(&bin_rel, "rel-bin").unwrap();

    let make_args = |profile: &'static str| FreshnessCheckArgs {
        project_dir: pdir,
        profile,
        package: &None,
        features: &[],
        no_default_features: false,
        all_features: false,
        toolchain_version: "rustc 1.85.0",
    };

    FastBuildLock::record_state(&RecordStateArgs {
        project_dir: pdir,
        profile: "dev",
        package: &None,
        features: &[],
        no_default_features: false,
        all_features: false,
        toolchain_version: "rustc 1.85.0",
        action_count: 1,
        root_binary: Some(bin_dev.clone()),
        outputs: &[bin_dev],
        stderr: None,
    })
    .unwrap();

    FastBuildLock::record_state(&RecordStateArgs {
        project_dir: pdir,
        profile: "release",
        package: &None,
        features: &[],
        no_default_features: false,
        all_features: false,
        toolchain_version: "rustc 1.85.0",
        action_count: 1,
        root_binary: Some(bin_rel.clone()),
        outputs: &[bin_rel],
        stderr: None,
    })
    .unwrap();

    assert!(matches!(
        FastBuildLock::check_freshness(&make_args("dev")),
        FreshnessResult::Fresh { .. }
    ));
    assert!(matches!(
        FastBuildLock::check_freshness(&make_args("release")),
        FreshnessResult::Fresh { .. }
    ));
}
