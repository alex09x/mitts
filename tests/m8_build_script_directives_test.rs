/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::rust::{RustAdapter, RustBuildOptions};
use std::fs;
use std::process::Command;

#[test]
fn test_m8_build_script_rustc_cfg_dynamic_key_invalidation() {
    let root = tempfile::tempdir().unwrap();
    let cache_dir = tempfile::tempdir().unwrap();

    fs::create_dir_all(root.path().join("crates/cfg_app/src")).unwrap();
    fs::write(
        root.path().join("Cargo.toml"),
        r#"[workspace]
members = ["crates/cfg_app"]
resolver = "2"
"#,
    )
    .unwrap();

    fs::write(
        root.path().join("crates/cfg_app/Cargo.toml"),
        r#"[package]
name = "cfg_app"
version = "0.1.0"
edition = "2021"
"#,
    )
    .unwrap();

    fs::write(
        root.path().join("crates/cfg_app/build.rs"),
        r#"fn main() {
    println!("cargo:rerun-if-env-changed=FAST");
    if std::env::var("FAST").is_ok() {
        println!("cargo:rustc-cfg=fast");
    }
}
"#,
    )
    .unwrap();

    fs::write(
        root.path().join("crates/cfg_app/src/main.rs"),
        r#"fn main() {
    print!("{}", cfg!(fast));
}
"#,
    )
    .unwrap();

    // 1. Build without FAST -> prints false
    std::env::remove_var("FAST");
    let opt1 = RustBuildOptions {
        manifest_path: Some(root.path().join("Cargo.toml")),
        cache_dir: Some(cache_dir.path().to_path_buf()),
        ..Default::default()
    };
    let (_, res1) = RustAdapter::build_native(&opt1).expect("Build 1 failed");
    assert_eq!(res1.exit_code, 0);

    let bin = root.path().join("target/mitts/debug/cfg_app");
    let out1 = Command::new(&bin).output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out1.stdout),
        "false",
        "Initial build without FAST must evaluate cfg!(fast) to false"
    );

    // 2. Re-build with FAST=1 -> build.rs emits cargo:rustc-cfg=fast -> dynamic key includes new directive hash -> rebuilds -> prints true!
    std::env::set_var("FAST", "1");
    let opt2 = RustBuildOptions {
        manifest_path: Some(root.path().join("Cargo.toml")),
        cache_dir: Some(cache_dir.path().to_path_buf()),
        ..Default::default()
    };
    let (_, res2) = RustAdapter::build_native(&opt2).expect("Build 2 failed");
    assert_eq!(res2.exit_code, 0);

    let out2 = Command::new(&bin).output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out2.stdout),
        "true",
        "Rebuilding with FAST=1 must compile with cfg!(fast)=true"
    );

    // 3. Re-build without FAST -> reverts to first cached entry -> cache HIT -> prints false!
    std::env::remove_var("FAST");
    let opt3 = RustBuildOptions {
        manifest_path: Some(root.path().join("Cargo.toml")),
        cache_dir: Some(cache_dir.path().to_path_buf()),
        ..Default::default()
    };
    let (_, res3) = RustAdapter::build_native(&opt3).expect("Build 3 failed");
    assert_eq!(res3.exit_code, 0);

    let out3 = Command::new(&bin).output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out3.stdout),
        "false",
        "Reverting FAST must hit first cache entry and print false"
    );

    // 4. Re-build with FAST=1 again -> hits second cached entry -> cache HIT -> prints true!
    std::env::set_var("FAST", "1");
    let opt4 = RustBuildOptions {
        manifest_path: Some(root.path().join("Cargo.toml")),
        cache_dir: Some(cache_dir.path().to_path_buf()),
        ..Default::default()
    };
    let (_, res4) = RustAdapter::build_native(&opt4).expect("Build 4 failed");
    assert_eq!(res4.exit_code, 0);

    let out4 = Command::new(&bin).output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out4.stdout),
        "true",
        "Re-enabling FAST=1 must hit second cache entry and print true"
    );

    std::env::remove_var("FAST");
}
