/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::{BuildProgressCallback, BuildProgressEvent, RustAdapter, RustBuildOptions};
use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex};
use tempfile::TempDir;

fn create_version_app_workspace(root: &Path, version: &str) {
    fs::create_dir_all(root.join("crates/ver_app/src")).unwrap();

    fs::write(
        root.join("Cargo.toml"),
        r#"[workspace]
members = ["crates/ver_app"]
resolver = "2"
"#,
    )
    .unwrap();

    fs::write(
        root.join("crates/ver_app/Cargo.toml"),
        format!(
            r#"[package]
name = "ver_app"
version = "{}"
edition = "2021"
"#,
            version
        ),
    )
    .unwrap();

    fs::write(
        root.join("crates/ver_app/src/main.rs"),
        r#"fn main() {
    print!("VER:{}", env!("CARGO_PKG_VERSION"));
}
"#,
    )
    .unwrap();
}

#[test]
fn test_m8_pkg_version_env_dep_same_clone_and_cross_clone_hit() {
    let cache_dir = TempDir::new().unwrap();

    // 1. Initial build in Clone A (version 0.1.0)
    let clone_a = TempDir::new().unwrap();
    create_version_app_workspace(clone_a.path(), "0.1.0");

    let compiling_a1 = Arc::new(Mutex::new(Vec::new()));
    let cb_a1 = {
        let list = compiling_a1.clone();
        BuildProgressCallback(Arc::new(move |evt| {
            if let BuildProgressEvent::Compiling(name) = evt {
                list.lock().unwrap().push(name);
            }
        }))
    };

    let opt_a1 = RustBuildOptions {
        manifest_path: Some(clone_a.path().join("Cargo.toml")),
        cache_dir: Some(cache_dir.path().to_path_buf()),
        progress: Some(cb_a1),
        ..Default::default()
    };

    let (_, res_a1) = RustAdapter::build_native(&opt_a1).expect("Initial build failed");
    assert_eq!(res_a1.exit_code, 0);
    assert!(
        !compiling_a1.lock().unwrap().is_empty(),
        "First build must compile the crate"
    );

    let bin_a = clone_a.path().join("target/mitts/debug/ver_app");
    assert!(bin_a.is_file());
    let out_a1 = Command::new(&bin_a).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out_a1.stdout), "VER:0.1.0");

    // 2. Re-build in Clone A after clean (removing target/mitts) -> must be pure HIT!
    let _ = fs::remove_dir_all(clone_a.path().join("target/mitts"));
    let compiling_a2 = Arc::new(Mutex::new(Vec::new()));
    let cb_a2 = {
        let list = compiling_a2.clone();
        BuildProgressCallback(Arc::new(move |evt| {
            if let BuildProgressEvent::Compiling(name) = evt {
                list.lock().unwrap().push(name);
            }
        }))
    };

    let opt_a2 = RustBuildOptions {
        manifest_path: Some(clone_a.path().join("Cargo.toml")),
        cache_dir: Some(cache_dir.path().to_path_buf()),
        progress: Some(cb_a2),
        ..Default::default()
    };

    let (_, res_a2) = RustAdapter::build_native(&opt_a2).expect("Rebuild after clean failed");
    assert_eq!(res_a2.exit_code, 0);
    assert!(
        compiling_a2.lock().unwrap().is_empty(),
        "Rebuild after clean must be a pure cache HIT (not recompiled due to CARGO_PKG_VERSION env-dep mismatch)"
    );
    let out_a2 = Command::new(&bin_a).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out_a2.stdout), "VER:0.1.0");

    // 3. Fresh Clone B (separate worktree, empty target, same CAS) -> must be pure cross-worktree HIT!
    let clone_b = TempDir::new().unwrap();
    create_version_app_workspace(clone_b.path(), "0.1.0");

    let compiling_b = Arc::new(Mutex::new(Vec::new()));
    let cb_b = {
        let list = compiling_b.clone();
        BuildProgressCallback(Arc::new(move |evt| {
            if let BuildProgressEvent::Compiling(name) = evt {
                list.lock().unwrap().push(name);
            }
        }))
    };

    let opt_b = RustBuildOptions {
        manifest_path: Some(clone_b.path().join("Cargo.toml")),
        cache_dir: Some(cache_dir.path().to_path_buf()),
        progress: Some(cb_b),
        ..Default::default()
    };

    let (_, res_b) = RustAdapter::build_native(&opt_b).expect("Clone B build failed");
    assert_eq!(res_b.exit_code, 0);
    assert!(
        compiling_b.lock().unwrap().is_empty(),
        "Clone B must achieve a cross-worktree CAS HIT for env!(\"CARGO_PKG_VERSION\")"
    );

    let bin_b = clone_b.path().join("target/mitts/debug/ver_app");
    assert!(bin_b.is_file());
    let out_b = Command::new(&bin_b).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out_b.stdout), "VER:0.1.0");

    // 4. Change version in Clone B to 0.2.0 -> must be a cache MISS and recompile
    fs::write(
        clone_b.path().join("crates/ver_app/Cargo.toml"),
        r#"[package]
name = "ver_app"
version = "0.2.0"
edition = "2021"
"#,
    )
    .unwrap();

    let compiling_b2 = Arc::new(Mutex::new(Vec::new()));
    let cb_b2 = {
        let list = compiling_b2.clone();
        BuildProgressCallback(Arc::new(move |evt| {
            if let BuildProgressEvent::Compiling(name) = evt {
                list.lock().unwrap().push(name);
            }
        }))
    };

    let opt_b2 = RustBuildOptions {
        manifest_path: Some(clone_b.path().join("Cargo.toml")),
        cache_dir: Some(cache_dir.path().to_path_buf()),
        progress: Some(cb_b2),
        ..Default::default()
    };

    let (_, res_b2) = RustAdapter::build_native(&opt_b2).expect("Clone B build v0.2.0 failed");
    assert_eq!(res_b2.exit_code, 0);
    assert!(
        !compiling_b2.lock().unwrap().is_empty(),
        "Version change to 0.2.0 must trigger a cache MISS and recompilation"
    );

    let out_b2 = Command::new(&bin_b).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out_b2.stdout), "VER:0.2.0");
}
