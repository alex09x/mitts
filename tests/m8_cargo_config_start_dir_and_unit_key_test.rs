/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::rust::key::derive_unit_action_keys;
use mitts::adapter::rust::types::RustBuildOptions;
use mitts::adapter::RustAdapter;
use mitts::core::build_lock::{
    FastBuildLock, FreshnessCheckArgs, FreshnessResult, RecordStateArgs,
};
use std::fs;
use std::sync::Mutex;
use tempfile::TempDir;

static SERIAL_TEST_LOCK: Mutex<()> = Mutex::new(());

struct EnvGuard {
    cargo_home_orig: Option<std::ffi::OsString>,
}

impl EnvGuard {
    fn new(isolated_cargo_home: &std::path::Path) -> Self {
        let cargo_home_orig = std::env::var_os("CARGO_HOME");
        std::env::set_var("CARGO_HOME", isolated_cargo_home);
        std::env::remove_var("CARGO_ENCODED_RUSTFLAGS");
        std::env::remove_var("RUSTFLAGS");
        std::env::remove_var("CARGO_BUILD_RUSTFLAGS");
        let host = mitts::adapter::rust::compiler_resolve::resolve_host_triple();
        let upper_triple = host.replace('-', "_").to_uppercase();
        std::env::remove_var(format!("CARGO_TARGET_{}_RUSTFLAGS", upper_triple));
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
        std::env::remove_var("CARGO_BUILD_RUSTFLAGS");
        let host = mitts::adapter::rust::compiler_resolve::resolve_host_triple();
        let upper_triple = host.replace('-', "_").to_uppercase();
        std::env::remove_var(format!("CARGO_TARGET_{}_RUSTFLAGS", upper_triple));
    }
}

struct WorkspaceFixture {
    _temp: TempDir,
    root: std::path::PathBuf,
    member: std::path::PathBuf,
    member_cfg: std::path::PathBuf,
}

impl WorkspaceFixture {
    fn new() -> Self {
        let temp = TempDir::new().unwrap();
        let root = temp.path().to_path_buf();

        fs::write(
            root.join("Cargo.toml"),
            r#"
[workspace]
members = ["member"]
resolver = "2"
"#,
        )
        .unwrap();

        let member = root.join("member");
        fs::create_dir_all(member.join("src")).unwrap();
        fs::write(
            member.join("Cargo.toml"),
            r#"
[package]
name = "member"
version = "0.1.0"
edition = "2021"

[lib]
path = "src/lib.rs"
"#,
        )
        .unwrap();
        fs::write(
            member.join("src/lib.rs"),
            "pub fn member_func() -> u32 { 42 }\n",
        )
        .unwrap();

        let member_cargo = member.join(".cargo");
        fs::create_dir_all(&member_cargo).unwrap();
        let member_cfg = member_cargo.join("config.toml");
        fs::write(
            &member_cfg,
            r#"
[build]
rustflags = ["--cfg", "member_cfg"]
"#,
        )
        .unwrap();

        Self {
            _temp: temp,
            root,
            member,
            member_cfg,
        }
    }
}

#[test]
fn test_member_config_yields_distinct_action_keys_from_root_vs_member() {
    let _lock = SERIAL_TEST_LOCK.lock().unwrap();
    let fix = WorkspaceFixture::new();
    let _guard = EnvGuard::new(&fix.root.join("empty_cargo_home"));

    // 1. Derive keys when invoked from workspace root (start_dir = root)
    let opt_root = RustBuildOptions {
        manifest_path: Some(fix.root.join("Cargo.toml")),
        start_dir: Some(fix.root.clone()),
        ..Default::default()
    };
    let plan_root = RustAdapter::explain_plan(&opt_root).expect("Plan from root failed");
    let keys_from_root =
        derive_unit_action_keys(&opt_root, &plan_root).expect("Keys from root failed");

    let member_unit = plan_root
        .units
        .iter()
        .find(|u| u.package_name == "member")
        .expect("Member unit not found in plan");
    let key_root = keys_from_root
        .get(&member_unit.unit_id())
        .expect("Member unit key missing in root invocation")
        .clone();

    // 2. Derive keys when invoked from member (start_dir = member)
    let opt_member = RustBuildOptions {
        manifest_path: Some(fix.root.join("Cargo.toml")),
        start_dir: Some(fix.member.clone()),
        ..Default::default()
    };
    let plan_member = RustAdapter::explain_plan(&opt_member).expect("Plan from member failed");
    let keys_from_member =
        derive_unit_action_keys(&opt_member, &plan_member).expect("Keys from member failed");

    let key_member = keys_from_member
        .get(&member_unit.unit_id())
        .expect("Member unit key missing in member invocation")
        .clone();

    // The key derived when invoked from member MUST differ from when invoked from root
    // because member/.cargo/config.toml defines custom rustflags.
    assert_ne!(
        key_root, key_member,
        "Action key for member invoked from root must NOT match action key invoked from member"
    );

    // 3. Modifying member config must produce a new distinct key
    fs::write(
        &fix.member_cfg,
        r#"
[build]
rustflags = ["--cfg", "member_cfg_v2"]
"#,
    )
    .unwrap();

    let keys_from_member_v2 =
        derive_unit_action_keys(&opt_member, &plan_member).expect("Keys v2 failed");
    let key_member_v2 = keys_from_member_v2
        .get(&member_unit.unit_id())
        .expect("Member unit key v2 missing")
        .clone();

    assert_ne!(
        key_member, key_member_v2,
        "Editing member/.cargo/config.toml must invalidate action key"
    );
    assert_ne!(
        key_root, key_member_v2,
        "V2 key must also differ from root key"
    );
}

#[test]
fn test_member_config_modification_invalidates_fast_build_lock() {
    let _lock = SERIAL_TEST_LOCK.lock().unwrap();
    let fix = WorkspaceFixture::new();
    let _guard = EnvGuard::new(&fix.root.join("empty_cargo_home"));

    let prev_cwd = std::env::current_dir().unwrap();
    std::env::set_current_dir(&fix.member).unwrap();

    let dummy_out = fix.member.join("dummy_artifact.rlib");
    fs::write(&dummy_out, b"artifact content").unwrap();

    let record_args = RecordStateArgs {
        project_dir: &fix.root,
        profile: "dev",
        package: &None,
        features: &[],
        no_default_features: false,
        all_features: false,
        toolchain_version: "rustc 1.85.0",
        action_count: 1,
        root_binary: None,
        outputs: std::slice::from_ref(&dummy_out),
        stderr: None,
    };
    FastBuildLock::record_state(&record_args).unwrap();

    let check_args = FreshnessCheckArgs {
        project_dir: &fix.root,
        profile: "dev",
        package: &None,
        features: &[],
        no_default_features: false,
        all_features: false,
        toolchain_version: "rustc 1.85.0",
    };

    // Baseline freshness check must be Fresh
    match FastBuildLock::check_freshness(&check_args) {
        FreshnessResult::Fresh { .. } => {}
        res => panic!("Expected Fresh, got {:?}", res),
    }

    // Modify member/.cargo/config.toml
    std::thread::sleep(std::time::Duration::from_millis(20));
    fs::write(
        &fix.member_cfg,
        r#"
[build]
rustflags = ["--cfg", "member_cfg_modified"]
"#,
    )
    .unwrap();

    // Fast build lock MUST detect modification
    match FastBuildLock::check_freshness(&check_args) {
        FreshnessResult::Stale(ref reason) => {
            assert!(
                reason.contains("Cargo config modified"),
                "Expected config modified reason, got: {}",
                reason
            );
        }
        res => panic!("Expected Stale on config modification, got {:?}", res),
    }

    std::env::set_current_dir(prev_cwd).unwrap();
}
