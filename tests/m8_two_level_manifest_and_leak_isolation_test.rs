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
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

fn create_workspace_with_external_asset(root: &Path, asset_content: &str) {
    fs::create_dir_all(root.join("assets")).unwrap();
    fs::create_dir_all(root.join("app/src")).unwrap();

    fs::write(
        root.join("Cargo.toml"),
        r#"[workspace]
members = ["app"]
resolver = "2"
"#,
    )
    .unwrap();

    fs::write(root.join("assets/x.txt"), asset_content).unwrap();

    fs::write(
        root.join("app/Cargo.toml"),
        r#"[package]
name = "app"
version = "0.1.0"
edition = "2021"
"#,
    )
    .unwrap();

    fs::write(
        root.join("app/src/main.rs"),
        r#"fn main() {
    let s = include_str!("../../assets/x.txt");
    print!("ASSET:{}", s);
}
"#,
    )
    .unwrap();
}

#[test]
fn test_m8_cross_worktree_include_str_manifest_reuse_and_invalidation() {
    let cache_dir = TempDir::new().unwrap();
    let cache_path = cache_dir.path().to_str().unwrap();

    let clone_a = TempDir::new().unwrap();
    create_workspace_with_external_asset(clone_a.path(), "v1");

    let opt_a = RustBuildOptions {
        manifest_path: Some(clone_a.path().join("Cargo.toml")),
        cache_dir: Some(cache_dir.path().to_path_buf()),
        ..Default::default()
    };
    std::env::set_var("MITTS_CACHE_DIR", cache_path);

    // 1. Build in Clone A (v1)
    let (_plan_a, res_a) = RustAdapter::build(&opt_a).expect("Clone A build failed");
    assert!(res_a.success, "Build failed with stderr: {}", res_a.stderr);
    let bin_a = clone_a.path().join("target/mitts/debug/app");
    assert!(bin_a.is_file());
    let out_a = Command::new(&bin_a).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out_a.stdout), "ASSET:v1");

    // 2. Build in Clone B (v2) - must NOT get a stale hit from Clone A
    let clone_b = TempDir::new().unwrap();
    create_workspace_with_external_asset(clone_b.path(), "v2");

    let opt_b = RustBuildOptions {
        manifest_path: Some(clone_b.path().join("Cargo.toml")),
        cache_dir: Some(cache_dir.path().to_path_buf()),
        ..Default::default()
    };
    let (plan_b, res_b) = RustAdapter::build(&opt_b).expect("Clone B build failed");
    assert!(res_b.success, "Build failed with stderr: {}", res_b.stderr);
    let bin_b = clone_b.path().join("target/mitts/debug/app");
    assert!(bin_b.is_file());
    let out_b = Command::new(&bin_b).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out_b.stdout), "ASSET:v2");

    // Verify Clone B recompiled and did not hit v1
    let restored_in_b = plan_b
        .actions
        .iter()
        .any(|a| a.description.contains("Restored app"));
    assert!(
        !restored_in_b,
        "Clone B with changed x.txt must NOT restore v1 from CAS"
    );

    // 3. Build in Clone C (v1) - must HIT Clone A's cached build
    let clone_c = TempDir::new().unwrap();
    create_workspace_with_external_asset(clone_c.path(), "v1");

    let opt_c = RustBuildOptions {
        manifest_path: Some(clone_c.path().join("Cargo.toml")),
        cache_dir: Some(cache_dir.path().to_path_buf()),
        ..Default::default()
    };
    let (plan_c, res_c) = RustAdapter::build(&opt_c).expect("Clone C build failed");
    assert!(res_c.success);
    let bin_c = clone_c.path().join("target/mitts/debug/app");
    assert!(bin_c.is_file());
    let out_c = Command::new(&bin_c).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out_c.stdout), "ASSET:v1");

    let restored_in_c = plan_c
        .actions
        .iter()
        .any(|a| a.description.contains("Restored app"));
    assert!(
        restored_in_c,
        "Clone C with identical x.txt as Clone A must achieve CAS cache hit"
    );

    // 4. In Clone B: mitts clean, change x.txt to v3, build -> must compile and print v3
    let target_b = clone_b.path().join("target");
    let _ = fs::remove_dir_all(&target_b);
    fs::write(clone_b.path().join("assets/x.txt"), "v3").unwrap();

    let (_plan_b3, res_b3) = RustAdapter::build(&opt_b).expect("Clone B3 build failed");
    assert!(res_b3.success);
    let bin_b3 = clone_b.path().join("target/mitts/debug/app");
    assert!(bin_b3.is_file());
    let out_b3 = Command::new(&bin_b3).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out_b3.stdout), "ASSET:v3");
}

fn create_workspace_with_impure_macro(root: &Path) {
    fs::create_dir_all(root.join("leak_macro/src")).unwrap();
    fs::create_dir_all(root.join("leak_app/src")).unwrap();

    fs::write(
        root.join("Cargo.toml"),
        r#"[workspace]
members = ["leak_macro", "leak_app"]
resolver = "2"
"#,
    )
    .unwrap();

    fs::write(
        root.join("leak_macro/Cargo.toml"),
        r#"[package]
name = "leak_macro"
version = "0.1.0"
edition = "2021"

[lib]
proc-macro = true
"#,
    )
    .unwrap();

    fs::write(
        root.join("leak_macro/src/lib.rs"),
        r#"use proc_macro::TokenStream;

#[proc_macro]
pub fn leak_dir(_input: TokenStream) -> TokenStream {
    let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| "UNKNOWN".into());
    format!("\"{}\"", dir).parse().unwrap()
}
"#,
    )
    .unwrap();

    fs::write(
        root.join("leak_app/Cargo.toml"),
        r#"[package]
name = "leak_app"
version = "0.1.0"
edition = "2021"

[dependencies]
leak_macro = { path = "../leak_macro" }
"#,
    )
    .unwrap();

    fs::write(
        root.join("leak_app/src/main.rs"),
        r#"fn main() {
    let leaked = leak_macro::leak_dir!();
    print!("LEAKED:{}", leaked);
}
"#,
    )
    .unwrap();
}

#[test]
fn test_m8_ws_leak_isolation_across_clones_and_hit_in_same_clone() {
    let cache_dir = TempDir::new().unwrap();
    let cache_path = cache_dir.path().to_str().unwrap();
    std::env::set_var("MITTS_CACHE_DIR", cache_path);

    let clone1 = TempDir::new().unwrap();
    create_workspace_with_impure_macro(clone1.path());

    let opt1 = RustBuildOptions {
        manifest_path: Some(clone1.path().join("Cargo.toml")),
        cache_dir: Some(cache_dir.path().to_path_buf()),
        ..Default::default()
    };

    // 1. Initial build in Clone 1
    let (_plan1, res1) = RustAdapter::build(&opt1).expect("Clone 1 build failed");
    assert!(res1.success, "Build 1 failed with stderr: {}", res1.stderr);
    let bin1 = clone1.path().join("target/mitts/debug/leak_app");
    assert!(bin1.is_file());
    let out1 = Command::new(&bin1).output().unwrap();
    let str1 = String::from_utf8_lossy(&out1.stdout);
    assert!(
        str1.contains(&clone1.path().to_string_lossy().to_string()),
        "Output must contain Clone 1's path"
    );

    // 2. Repeat build in same Clone 1 -> must achieve cache hit!
    let (plan1_repeat, res1_repeat) = RustAdapter::build(&opt1).expect("Repeat build failed");
    assert!(res1_repeat.success);
    let restored_repeat = plan1_repeat
        .actions
        .iter()
        .any(|a| a.description.contains("Restored leak_app"));
    assert!(
        restored_repeat,
        "Repeat build in the same worktree must achieve CAS cache hit for leaky unit"
    );

    // 3. Build in Clone 2 (different worktree) -> must NOT hit Clone 1's leaky artifact
    let clone2 = TempDir::new().unwrap();
    create_workspace_with_impure_macro(clone2.path());

    let opt2 = RustBuildOptions {
        manifest_path: Some(clone2.path().join("Cargo.toml")),
        cache_dir: Some(cache_dir.path().to_path_buf()),
        ..Default::default()
    };
    let (_plan2, res2) = RustAdapter::build(&opt2).expect("Clone 2 build failed");
    assert!(res2.success, "Build 2 failed with stderr: {}", res2.stderr);
    let bin2 = clone2.path().join("target/mitts/debug/leak_app");
    assert!(bin2.is_file());
    let out2 = Command::new(&bin2).output().unwrap();
    let str2 = String::from_utf8_lossy(&out2.stdout);
    assert!(
        str2.contains(&clone2.path().to_string_lossy().to_string()),
        "Clone 2 output must contain Clone 2's path, not Clone 1's"
    );
    assert!(
        !str2.contains(&clone1.path().to_string_lossy().to_string()),
        "Clone 2 output must NOT leak Clone 1's path"
    );
}
