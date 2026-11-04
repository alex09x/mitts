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
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn create_worktree(base: &Path, name: &str) -> PathBuf {
    let root = base.join(name);
    fs::create_dir_all(root.join("src")).unwrap();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/hello-rust");
    for path in ["Cargo.toml", "Cargo.lock", "src/main.rs"] {
        fs::copy(fixture.join(path), root.join(path)).unwrap();
    }
    root
}

#[test]
fn cold_warm_and_cross_worktree_builds_reuse_artifacts() {
    let temp = tempfile::tempdir().unwrap();
    let first = create_worktree(temp.path(), "first");
    let cache = temp.path().join("cache");
    let options = RustBuildOptions {
        manifest_path: Some(first.join("Cargo.toml")),
        cache_dir: Some(cache.clone()),
        target_dir: Some(first.join("target")),
        native: true,
        ..Default::default()
    };
    let (_, cold) = RustAdapter::build_native(&options).unwrap();
    assert!(cold.success && !cold.cached);
    fs::remove_dir_all(first.join("target")).unwrap();
    let (_, warm) = RustAdapter::build_native(&options).unwrap();
    assert!(warm.success && warm.cached);
    let second = create_worktree(temp.path(), "second");
    let other = RustBuildOptions {
        manifest_path: Some(second.join("Cargo.toml")),
        cache_dir: Some(cache.clone()),
        target_dir: Some(second.join("target")),
        native: true,
        ..Default::default()
    };
    assert!(RustAdapter::build_native(&other).unwrap().1.success);
    let count = fs::read_dir(cache.join("cas/artifacts"))
        .unwrap()
        .flatten()
        .filter(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            name.starts_with("libitoa") && name.ends_with(".rlib")
        })
        .count();
    assert_eq!(count, 1);
    for root in [first, second] {
        let output = Command::new(root.join("target/debug/hello-rust"))
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).contains("Hello from Mitts fixture: 42"));
    }
}
