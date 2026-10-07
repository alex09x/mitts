/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::{RustAdapter, RustBuildOptions};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn fixture_manifest() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("hello-rust")
        .join("Cargo.toml")
}

fn resolve_sccache_bin() -> Option<PathBuf> {
    if let Ok(output) = Command::new("which").arg("sccache").output() {
        if output.status.success() {
            let path_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path_str.is_empty() {
                return Some(PathBuf::from(path_str));
            }
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        let p = PathBuf::from(home).join(".cargo").join("bin").join("sccache");
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

fn create_isolated_worktree(base_dir: &Path, name: &str) -> PathBuf {
    let wt_dir = base_dir.join(name);
    fs::create_dir_all(wt_dir.join("src")).unwrap();
    let fixture_dir = fixture_manifest().parent().unwrap().to_path_buf();
    fs::copy(fixture_dir.join("Cargo.toml"), wt_dir.join("Cargo.toml")).unwrap();
    fs::copy(fixture_dir.join("Cargo.lock"), wt_dir.join("Cargo.lock")).unwrap();
    fs::copy(
        fixture_dir.join("src").join("main.rs"),
        wt_dir.join("src").join("main.rs"),
    )
    .unwrap();
    wt_dir
}

#[test]
fn test_m5_three_way_comparison_cargo_alone_vs_sccache_vs_mitts_cold() {
    let temp_root = tempfile::tempdir().expect("temp root");
    let base_dir = temp_root.path();

    // 1. Pure Cargo (Alone)
    let wt_cargo = create_isolated_worktree(base_dir, "wt_cargo");
    let cargo_target = wt_cargo.join("target");
    let status_cargo = Command::new("cargo")
        .arg("build")
        .current_dir(&wt_cargo)
        .env("RUSTC_WRAPPER", "")
        .env("CARGO_TARGET_DIR", &cargo_target)
        .status()
        .expect("cargo build (alone)");
    assert!(status_cargo.success(), "Pure Cargo build must succeed");

    let cargo_bin = cargo_target.join("debug").join("hello-rust");
    assert!(cargo_bin.is_file(), "Cargo binary must exist");
    let cargo_out = Command::new(&cargo_bin).output().unwrap();
    assert!(String::from_utf8_lossy(&cargo_out.stdout).contains("Hello from Mitts fixture: 42"));

    // 2. Cargo with sccache
    if let Some(sccache_bin) = resolve_sccache_bin() {
        let wt_sccache = create_isolated_worktree(base_dir, "wt_sccache");
        let sccache_target = wt_sccache.join("target");
        let sccache_cache_dir = base_dir.join("sccache_cache");
        fs::create_dir_all(&sccache_cache_dir).unwrap();

        let _ = Command::new(&sccache_bin).arg("--stop-server").output();

        let status_sccache = Command::new("cargo")
            .arg("build")
            .current_dir(&wt_sccache)
            .env("RUSTC_WRAPPER", &sccache_bin)
            .env("SCCACHE_DIR", &sccache_cache_dir)
            .env("CARGO_TARGET_DIR", &sccache_target)
            .status()
            .expect("cargo build with sccache");
        assert!(status_sccache.success(), "Cargo with sccache must succeed");

        let sccache_bin_out = sccache_target.join("debug").join("hello-rust");
        assert!(sccache_bin_out.is_file(), "Cargo+sccache binary must exist");
        let sc_out = Command::new(&sccache_bin_out).output().unwrap();
        assert!(String::from_utf8_lossy(&sc_out.stdout).contains("Hello from Mitts fixture: 42"));

        let _ = Command::new(&sccache_bin).arg("--stop-server").output();
    } else {
        eprintln!("sccache not installed on host, skipping Cargo+sccache branch");
    }

    // 3. Mitts Build (Single Cold)
    let wt_mitts = create_isolated_worktree(base_dir, "wt_mitts");
    let mitts_cas_dir = base_dir.join("mitts_cas");
    let opt_mitts = RustBuildOptions {
        manifest_path: Some(wt_mitts.join("Cargo.toml")),
        cache_dir: Some(mitts_cas_dir),
        target_dir: Some(wt_mitts.join("target")),
        native: true,
        ..Default::default()
    };
    let (_, res_mitts) = RustAdapter::build_native(&opt_mitts).expect("mitts build");
    assert!(res_mitts.success, "Mitts build must succeed");
    assert!(!res_mitts.cached, "Mitts first build must be cold");

    let mitts_bin = wt_mitts.join("target").join("debug").join("hello-rust");
    assert!(mitts_bin.is_file(), "Mitts binary must exist");
    let mitts_out = Command::new(&mitts_bin).output().unwrap();
    assert!(String::from_utf8_lossy(&mitts_out.stdout).contains("Hello from Mitts fixture: 42"));
}

#[test]
fn test_m5_three_way_comparison_cargo_alone_vs_sccache_vs_mitts_warm() {
    let temp_root = tempfile::tempdir().expect("temp root");
    let base_dir = temp_root.path();

    // Seed Mitts CAS and sccache with worktree 1
    let wt1 = create_isolated_worktree(base_dir, "wt1");
    let shared_cas = base_dir.join("shared_cas");
    let opt_seed = RustBuildOptions {
        manifest_path: Some(wt1.join("Cargo.toml")),
        cache_dir: Some(shared_cas.clone()),
        target_dir: Some(wt1.join("target")),
        native: true,
        ..Default::default()
    };
    let (_, seed_res) = RustAdapter::build_native(&opt_seed).expect("seed mitts build");
    assert!(seed_res.success);

    // 1. Warm build on wt1 after wiping local target directory (simulating clean target with shared CAS)
    let _ = fs::remove_dir_all(wt1.join("target"));
    let (_, warm_res) = RustAdapter::build_native(&opt_seed).expect("warm mitts build");
    assert!(warm_res.success);
    assert!(
        warm_res.cached,
        "Mitts build with clean target must be 100% warm action cache hit"
    );

    let mitts_bin1 = wt1.join("target").join("debug").join("hello-rust");
    assert!(mitts_bin1.is_file());
    let out1 = Command::new(&mitts_bin1).output().unwrap();
    assert!(String::from_utf8_lossy(&out1.stdout).contains("Hello from Mitts fixture: 42"));

    // 2. Cross-worktree build in wt2 sharing the same CAS (dependency cache hit)
    let wt2 = create_isolated_worktree(base_dir, "wt2");
    let opt_wt2 = RustBuildOptions {
        manifest_path: Some(wt2.join("Cargo.toml")),
        cache_dir: Some(shared_cas.clone()),
        target_dir: Some(wt2.join("target")),
        native: true,
        ..Default::default()
    };
    let (_, wt2_res) = RustAdapter::build_native(&opt_wt2).expect("wt2 build");
    assert!(wt2_res.success);

    // Verify itoa was reused from CAS and exists exactly once
    let cas_artifacts = shared_cas.join("cas").join("artifacts");
    let itoa_count = fs::read_dir(&cas_artifacts)
        .unwrap()
        .flatten()
        .filter(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            name.starts_with("libitoa") && name.ends_with(".rlib")
        })
        .count();
    assert_eq!(itoa_count, 1, "itoa must be shared and cached in CAS");

    let mitts_bin2 = wt2.join("target").join("debug").join("hello-rust");
    assert!(mitts_bin2.is_file());
    let out2 = Command::new(&mitts_bin2).output().unwrap();
    assert!(String::from_utf8_lossy(&out2.stdout).contains("Hello from Mitts fixture: 42"));
}
