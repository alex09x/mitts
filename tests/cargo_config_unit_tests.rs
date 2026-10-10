/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::rust::cargo_config::*;
use std::fs;
use std::path::Path;
use tempfile::tempdir;

static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct EnvGuard {
    cargo_home_orig: Option<std::ffi::OsString>,
}

impl EnvGuard {
    fn new(isolated_cargo_home: &Path) -> Self {
        let cargo_home_orig = std::env::var_os("CARGO_HOME");
        std::env::set_var("CARGO_HOME", isolated_cargo_home);
        Self { cargo_home_orig }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        if let Some(ref orig) = self.cargo_home_orig {
            std::env::set_var("CARGO_HOME", orig);
        } else {
            std::env::remove_var("CARGO_HOME");
        }
    }
}

#[test]
fn test_load_cargo_config_rustflags_from_dir() {
    let _lock = TEST_LOCK.lock().unwrap();
    let home = tempdir().unwrap();
    let _guard = EnvGuard::new(home.path());

    let dir = tempdir().unwrap();
    let cargo_dir = dir.path().join(".cargo");
    fs::create_dir_all(&cargo_dir).unwrap();
    fs::write(
        cargo_dir.join("config.toml"),
        "[build]\nrustflags = [\"--cfg\", \"test_loaded\"]\n",
    )
    .unwrap();

    let loaded = load_cargo_config_rustflags(dir.path(), None).unwrap();
    assert!(loaded.contains(&"--cfg".to_string()));
    assert!(loaded.contains(&"test_loaded".to_string()));
}

#[test]
fn test_bare_config_precedence_over_config_toml() {
    let _lock = TEST_LOCK.lock().unwrap();
    let home = tempdir().unwrap();
    let _guard = EnvGuard::new(home.path());

    let dir = tempdir().unwrap();
    let cargo_dir = dir.path().join(".cargo");
    fs::create_dir_all(&cargo_dir).unwrap();
    fs::write(
        cargo_dir.join("config"),
        "[build]\nrustflags = [\"--cfg\", \"from_bare_config\"]\n",
    )
    .unwrap();
    fs::write(
        cargo_dir.join("config.toml"),
        "[build]\nrustflags = [\"--cfg\", \"from_config_toml\"]\n",
    )
    .unwrap();

    let files = discover_cargo_config_files(dir.path());
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].file_name().unwrap(), "config");

    let loaded = load_cargo_config_rustflags(dir.path(), None).unwrap();
    assert_eq!(loaded, vec!["--cfg", "from_bare_config"]);
}

#[test]
fn test_target_overrides_build_across_files() {
    let _lock = TEST_LOCK.lock().unwrap();
    let home = tempdir().unwrap();
    let _guard = EnvGuard::new(home.path());

    let parent = tempdir().unwrap();
    let child = parent.path().join("child");
    fs::create_dir_all(&child).unwrap();

    let parent_cargo = parent.path().join(".cargo");
    fs::create_dir_all(&parent_cargo).unwrap();
    fs::write(
        parent_cargo.join("config.toml"),
        "[build]\nrustflags = [\"--cfg\", \"parent_build\"]\n",
    )
    .unwrap();

    let child_cargo = child.join(".cargo");
    fs::create_dir_all(&child_cargo).unwrap();
    fs::write(
        child_cargo.join("config.toml"),
        "[target.x86_64-unknown-linux-gnu]\nrustflags = [\"--cfg\", \"child_target\"]\n",
    )
    .unwrap();

    let loaded = load_cargo_config_rustflags(&child, Some("x86_64-unknown-linux-gnu")).unwrap();
    assert_eq!(loaded, vec!["--cfg", "child_target"]);
}

#[test]
fn test_load_cargo_config_build_target_string_and_array() {
    let _lock = TEST_LOCK.lock().unwrap();
    let home = tempdir().unwrap();
    let _guard = EnvGuard::new(home.path());

    let dir = tempdir().unwrap();
    let cargo_dir = dir.path().join(".cargo");
    fs::create_dir_all(&cargo_dir).unwrap();
    fs::write(
        cargo_dir.join("config.toml"),
        "[build]\ntarget = \"wasm32-unknown-unknown\"\n",
    )
    .unwrap();

    let target = load_cargo_config_build_target(dir.path()).unwrap();
    assert_eq!(target.as_deref(), Some("wasm32-unknown-unknown"));

    fs::write(
        cargo_dir.join("config.toml"),
        "[build]\ntarget = [\"aarch64-apple-darwin\", \"x86_64-apple-darwin\"]\n",
    )
    .unwrap();

    let target_arr = load_cargo_config_build_target(dir.path()).unwrap();
    assert_eq!(target_arr.as_deref(), Some("aarch64-apple-darwin"));
}
