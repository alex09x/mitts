/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::rust::features::{resolve_package_features, PackageFeatureContext};
use mitts::adapter::RustBuildOptions;
use mitts::cli::handlers::handle_clean;
use mitts::cli::CleanArgs;
use mitts::core::build_lock::FastBuildLock;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use tempfile::TempDir;

static ENV_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn test_m8_clean_cli_release_and_cache_flags() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = TempDir::new().unwrap();
    let pkg_dir = temp.path().join("clean_pkg");
    let mitts_dir = pkg_dir.join("target").join("mitts");
    let rel_target = mitts_dir.join("release");
    fs::create_dir_all(&rel_target).unwrap();

    fs::write(
        pkg_dir.join("Cargo.toml"),
        br#"[package]
name = "clean_pkg"
version = "0.1.0"
edition = "2021"
"#,
    )
    .unwrap();

    // 1. handle_clean with nonexistent manifest returns 1
    let args_invalid = CleanArgs {
        manifest_path: Some(PathBuf::from("/nonexistent/Cargo.toml")),
        ..Default::default()
    };
    assert_eq!(handle_clean(&args_invalid), 1);

    // Create release fast lock and log
    let rel_lock = FastBuildLock::lock_path(&pkg_dir, "release");
    fs::write(&rel_lock, b"lock").unwrap();
    fs::write(mitts_dir.join("build.log"), b"log").unwrap();
    fs::write(rel_target.join("my_bin"), b"binary").unwrap();

    // 2. handle_clean with release: true removes only release target and release lock
    let args_release = CleanArgs {
        manifest_path: Some(pkg_dir.join("Cargo.toml")),
        release: true,
        cache: false,
        cache_dir: None,
    };
    assert_eq!(handle_clean(&args_release), 0);
    assert!(!rel_lock.exists());
    assert!(!rel_target.exists());

    // 3. handle_clean with cache: true prunes global cache
    let cache_dir = temp.path().join("cache");
    let args_cache = CleanArgs {
        manifest_path: Some(pkg_dir.join("Cargo.toml")),
        release: false,
        cache: true,
        cache_dir: Some(cache_dir),
    };
    assert_eq!(handle_clean(&args_cache), 0);
}

#[test]
fn test_m8_features_all_features_weak_deps_and_target_deps() {
    let toml_str = r#"
[package]
name = "feat_pkg"
version = "0.1.0"
edition = "2021"

[features]
default = ["std"]
std = ["dep:serde"]
extra = ["serde?/derive"]
custom = []

[dependencies]
serde = { version = "1.0", optional = true }

[target.'cfg(unix)'.dependencies]
libc = { version = "0.2", optional = true }
"#;

    let toml = mitts::adapter::rust::manifest::CargoToml::parse(toml_str).unwrap();
    let deps = vec!["serde".to_string()];

    let ctx = PackageFeatureContext {
        name: "feat_pkg",
        version: "0.1.0",
        source: None,
        is_workspace: true,
        is_root: true,
        toml: &toml,
        dependencies: &deps,
    };

    let packages = vec![ctx];

    // 1. Test opt.all_features = true
    let opt_all = RustBuildOptions {
        all_features: true,
        ..Default::default()
    };
    let feats_all = resolve_package_features(&packages, &opt_all).unwrap();
    let feat_list = feats_all.get("feat_pkg@0.1.0").unwrap();
    assert!(feat_list.contains(&"extra".to_string()));
    assert!(feat_list.contains(&"custom".to_string()));
    assert!(feat_list.contains(&"std".to_string()));

    // 2. Weak dependency inactive (serde not active in dependencies)
    let toml_inactive = mitts::adapter::rust::manifest::CargoToml::parse(toml_str).unwrap();
    let empty_deps = Vec::new();
    let ctx_inactive = PackageFeatureContext {
        name: "feat_pkg",
        version: "0.1.0",
        source: None,
        is_workspace: true,
        is_root: true,
        toml: &toml_inactive,
        dependencies: &empty_deps,
    };
    let opt_single = RustBuildOptions {
        features: vec!["extra".to_string()],
        no_default_features: true,
        ..Default::default()
    };
    let feats_inactive = resolve_package_features(&[ctx_inactive], &opt_single).unwrap();
    let feat_inactive_list = feats_inactive.get("feat_pkg@0.1.0").unwrap();
    // Since serde is inactive and extra uses serde?/derive, serde is not activated
    assert!(!feat_inactive_list.contains(&"serde".to_string()));
}
