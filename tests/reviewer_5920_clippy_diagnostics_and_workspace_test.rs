/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::rust::manifest::CargoToml;
use mitts::adapter::rust::types::RustBuildOptions;
use mitts::adapter::{BuildMode, BuildProgressCallback, BuildProgressEvent, RustAdapter};
use std::fs;
use std::sync::{Arc, Mutex};
use tempfile::TempDir;

#[test]
fn test_clippy_full_ac_hit_single_diagnostic_replay() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    fs::write(
        root.join("Cargo.toml"),
        r#"[package]
name = "lint_pkg"
version = "0.1.0"
edition = "2021"
"#,
    )
    .unwrap();

    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir).unwrap();
    fs::write(
        src_dir.join("lib.rs"),
        r#"
#[allow(dead_code)]
pub fn unused_func() {
    let unused_test_var = 42;
}
"#,
    )
    .unwrap();

    let temp_cache = TempDir::new().unwrap();
    let cache_dir = temp_cache.path().to_path_buf();
    let target_dir = root.join("target");

    // Run 1: Cold run
    let events_cold = Arc::new(Mutex::new(Vec::new()));
    let ev_c = Arc::clone(&events_cold);
    let opt_cold = RustBuildOptions {
        manifest_path: Some(root.join("Cargo.toml")),
        cache_dir: Some(cache_dir.clone()),
        target_dir: Some(target_dir.clone()),
        mode: BuildMode::Clippy,
        native: true,
        progress: Some(BuildProgressCallback(Arc::new(move |ev| {
            if let BuildProgressEvent::CompilerMessage(msg) = ev {
                ev_c.lock().unwrap().push(msg);
            }
        }))),
        ..Default::default()
    };

    let res_cold = RustAdapter::build(&opt_cold);
    assert!(res_cold.is_ok(), "Cold clippy build must succeed");
    let (_, result_cold) = res_cold.unwrap();
    assert!(result_cold.success);

    // Run 2: Full AC hit
    let events_cached = Arc::new(Mutex::new(Vec::new()));
    let ev_w = Arc::clone(&events_cached);
    let opt_cached = RustBuildOptions {
        manifest_path: Some(root.join("Cargo.toml")),
        cache_dir: Some(cache_dir),
        target_dir: Some(target_dir),
        mode: BuildMode::Clippy,
        native: true,
        progress: Some(BuildProgressCallback(Arc::new(move |ev| {
            if let BuildProgressEvent::CompilerMessage(msg) = ev {
                ev_w.lock().unwrap().push(msg);
            }
        }))),
        ..Default::default()
    };

    let res_cached = RustAdapter::build(&opt_cached);
    assert!(res_cached.is_ok(), "Cached clippy build must succeed");
    let (_, result_cached) = res_cached.unwrap();
    assert!(result_cached.success);
    assert!(result_cached.cached, "Expected full action cache hit");

    let cached_msgs = events_cached.lock().unwrap();
    assert_eq!(
        cached_msgs.len(),
        1,
        "Expected exactly 1 CompilerMessage event on AC hit, got: {:?}",
        *cached_msgs
    );
    let count_summary = cached_msgs[0].matches("warning: 1 warning emitted").count();
    assert_eq!(
        count_summary, 1,
        "Expected exactly 1 warning summary line, got: {count_summary}\nMessage: {}",
        cached_msgs[0]
    );

    // Also assert that if result.stderr was captured, it matches single emission
    let count_in_stderr = result_cached
        .stderr
        .matches("warning: 1 warning emitted")
        .count();
    assert_eq!(
        count_in_stderr, 1,
        "result.stderr must contain warning exactly once, got: {count_in_stderr}"
    );
}

#[test]
fn test_discover_workspace_members_respects_exclude() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    fs::write(
        root.join("Cargo.toml"),
        r#"[workspace]
members = ["crates/*"]
exclude = ["crates/excluded"]
"#,
    )
    .unwrap();

    let inc_dir = root.join("crates").join("included");
    let exc_dir = root.join("crates").join("excluded");
    fs::create_dir_all(&inc_dir).unwrap();
    fs::create_dir_all(&exc_dir).unwrap();

    fs::write(
        inc_dir.join("Cargo.toml"),
        r#"[package]
name = "included"
version = "0.1.0"
edition = "2021"
"#,
    )
    .unwrap();

    fs::write(
        exc_dir.join("Cargo.toml"),
        r#"[package]
name = "excluded"
version = "0.1.0"
edition = "2021"
"#,
    )
    .unwrap();

    let ws_toml = CargoToml::from_file(&root.join("Cargo.toml")).unwrap();
    let members = ws_toml.discover_workspace_members(root).unwrap();

    let names: Vec<_> = members.iter().map(|(n, _)| n.as_str()).collect();
    assert!(names.contains(&"included"), "Expected included member");
    assert!(
        !names.contains(&"excluded"),
        "Expected excluded member to be ignored"
    );
}

#[test]
fn test_discover_workspace_members_malformed_toml_fails() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    fs::write(
        root.join("Cargo.toml"),
        r#"[workspace]
members = ["crates/*"]
"#,
    )
    .unwrap();

    let broken_dir = root.join("crates").join("broken");
    fs::create_dir_all(&broken_dir).unwrap();
    fs::write(broken_dir.join("Cargo.toml"), "this is NOT valid toml !!!").unwrap();

    let ws_toml = CargoToml::from_file(&root.join("Cargo.toml")).unwrap();
    let res = ws_toml.discover_workspace_members(root);
    assert!(
        res.is_err(),
        "Expected error for malformed member Cargo.toml"
    );
    let err_str = res.unwrap_err().to_string();
    assert!(err_str.contains("Failed to parse workspace member manifest"));
}

#[test]
fn test_discover_workspace_members_unsupported_pattern_returns_error() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    fs::write(
        root.join("Cargo.toml"),
        r#"[workspace]
members = ["crates/[a-z]*"]
"#,
    )
    .unwrap();

    let ws_toml = CargoToml::from_file(&root.join("Cargo.toml")).unwrap();
    let res = ws_toml.discover_workspace_members(root);
    assert!(
        res.is_err(),
        "Expected error for unsupported glob character class"
    );
    let err_str = res.unwrap_err().to_string();
    assert!(err_str.contains("glob character classes and `?` wildcards are not supported"));
}
