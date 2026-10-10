/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::rust::cargo_config::load_cargo_config_rustflags;
use mitts::adapter::rust::compiler::RustcInvocation;
use mitts::adapter::rust::compiler_args::build_compiler_command;
use mitts::adapter::rust::compiler_resolve::resolve_host_triple;
use mitts::adapter::rust::types::{BuildMode, PlannedCompilationUnit};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tempfile::TempDir;

static SERIAL_TEST_LOCK: Mutex<()> = Mutex::new(());

struct EnvGuard {
    cargo_home_orig: Option<std::ffi::OsString>,
}

impl EnvGuard {
    fn new(isolated_cargo_home: &Path) -> Self {
        let cargo_home_orig = std::env::var_os("CARGO_HOME");
        std::env::set_var("CARGO_HOME", isolated_cargo_home);
        std::env::remove_var("CARGO_ENCODED_RUSTFLAGS");
        std::env::remove_var("RUSTFLAGS");
        std::env::remove_var("CARGO_BUILD_RUSTFLAGS");
        let host = resolve_host_triple();
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
        let host = resolve_host_triple();
        let upper_triple = host.replace('-', "_").to_uppercase();
        std::env::remove_var(format!("CARGO_TARGET_{}_RUSTFLAGS", upper_triple));
    }
}

#[test]
fn test_cross_file_target_overrides_build_rustflags() {
    let _lock = SERIAL_TEST_LOCK.lock().unwrap();
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    let _guard = EnvGuard::new(&root.join("empty_cargo_home"));

    let parent_cargo = root.join(".cargo");
    fs::create_dir_all(&parent_cargo).unwrap();
    fs::write(
        parent_cargo.join("config.toml"),
        r#"
[build]
rustflags = ["--cfg", "parent_build_flag"]
"#,
    )
    .unwrap();

    let child = root.join("child");
    let child_cargo = child.join(".cargo");
    fs::create_dir_all(&child_cargo).unwrap();
    let host = resolve_host_triple();
    fs::write(
        child_cargo.join("config.toml"),
        format!(
            r#"
[target.{}]
rustflags = ["--cfg", "child_target_flag"]
"#,
            host
        ),
    )
    .unwrap();

    let loaded = load_cargo_config_rustflags(&child, None).unwrap();
    assert_eq!(
        loaded,
        vec!["--cfg", "child_target_flag"],
        "Target rustflags must completely override build rustflags across config files"
    );
}

#[test]
fn test_cargo_target_env_var_appended_to_file_target() {
    let _lock = SERIAL_TEST_LOCK.lock().unwrap();
    let temp = TempDir::new().unwrap();
    let child = temp.path();
    let _guard = EnvGuard::new(&child.join("empty_cargo_home"));

    let child_cargo = child.join(".cargo");
    fs::create_dir_all(&child_cargo).unwrap();
    let host = resolve_host_triple();
    fs::write(
        child_cargo.join("config.toml"),
        format!(
            r#"
[target.{}]
rustflags = ["--cfg", "from_target_config"]
"#,
            host
        ),
    )
    .unwrap();

    let upper_triple = host.replace('-', "_").to_uppercase();
    let target_env = format!("CARGO_TARGET_{}_RUSTFLAGS", upper_triple);

    std::env::set_var(&target_env, "--cfg from_target_env_active");
    let loaded = load_cargo_config_rustflags(child, None).unwrap();
    std::env::remove_var(&target_env);

    assert_eq!(
        loaded,
        vec![
            "--cfg",
            "from_target_config",
            "--cfg",
            "from_target_env_active"
        ],
        "CARGO_TARGET_<TRIPLE>_RUSTFLAGS must be appended to config file target flags"
    );
}

#[test]
fn test_cargo_build_env_var_appended_to_file_build() {
    let _lock = SERIAL_TEST_LOCK.lock().unwrap();
    let temp = TempDir::new().unwrap();
    let child = temp.path();
    let _guard = EnvGuard::new(&child.join("empty_cargo_home"));

    let child_cargo = child.join(".cargo");
    fs::create_dir_all(&child_cargo).unwrap();
    fs::write(
        child_cargo.join("config.toml"),
        r#"
[build]
rustflags = ["--cfg", "from_build_config"]
"#,
    )
    .unwrap();

    std::env::set_var("CARGO_BUILD_RUSTFLAGS", "--cfg from_build_env_active");
    let loaded = load_cargo_config_rustflags(child, None).unwrap();
    std::env::remove_var("CARGO_BUILD_RUSTFLAGS");

    assert_eq!(
        loaded,
        vec![
            "--cfg",
            "from_build_config",
            "--cfg",
            "from_build_env_active"
        ],
        "CARGO_BUILD_RUSTFLAGS must be appended to config file build flags"
    );
}

#[test]
fn test_target_in_config_overrides_cargo_build_env_var() {
    let _lock = SERIAL_TEST_LOCK.lock().unwrap();
    let temp = TempDir::new().unwrap();
    let child = temp.path();
    let _guard = EnvGuard::new(&child.join("empty_cargo_home"));

    let child_cargo = child.join(".cargo");
    fs::create_dir_all(&child_cargo).unwrap();
    let host = resolve_host_triple();
    fs::write(
        child_cargo.join("config.toml"),
        format!(
            r#"
[target.{}]
rustflags = ["--cfg", "from_target_config_active"]
"#,
            host
        ),
    )
    .unwrap();

    std::env::set_var("CARGO_BUILD_RUSTFLAGS", "--cfg from_build_env_active");
    let loaded = load_cargo_config_rustflags(child, None).unwrap();
    std::env::remove_var("CARGO_BUILD_RUSTFLAGS");

    assert_eq!(
        loaded,
        vec!["--cfg", "from_target_config_active"],
        "target.<triple>.rustflags in config must take precedence over CARGO_BUILD_RUSTFLAGS in environment"
    );
}

#[test]
fn test_empty_target_falls_back_to_build_flags() {
    let _lock = SERIAL_TEST_LOCK.lock().unwrap();
    let temp = TempDir::new().unwrap();
    let child = temp.path();
    let _guard = EnvGuard::new(&child.join("empty_cargo_home"));

    let child_cargo = child.join(".cargo");
    fs::create_dir_all(&child_cargo).unwrap();
    let host = resolve_host_triple();
    fs::write(
        child_cargo.join("config.toml"),
        format!(
            r#"
[build]
rustflags = ["--cfg", "fallback_build_flag"]

[target.{}]
rustflags = []
"#,
            host
        ),
    )
    .unwrap();

    let loaded = load_cargo_config_rustflags(child, None).unwrap();
    assert_eq!(
        loaded,
        vec!["--cfg", "fallback_build_flag"],
        "Empty target.<triple>.rustflags must fall back to build.rustflags matching Cargo"
    );
}

#[test]
fn test_bare_config_preferred_over_config_toml_in_same_dir() {
    let _lock = SERIAL_TEST_LOCK.lock().unwrap();
    let temp = TempDir::new().unwrap();
    let child = temp.path();
    let _guard = EnvGuard::new(&child.join("empty_cargo_home"));

    let child_cargo = child.join(".cargo");
    fs::create_dir_all(&child_cargo).unwrap();
    fs::write(
        child_cargo.join("config"),
        "[build]\nrustflags = [\"--cfg\", \"preferred_bare_config\"]\n",
    )
    .unwrap();
    fs::write(
        child_cargo.join("config.toml"),
        "[build]\nrustflags = [\"--cfg\", \"ignored_config_toml\"]\n",
    )
    .unwrap();

    let loaded = load_cargo_config_rustflags(child, None).unwrap();
    assert_eq!(
        loaded,
        vec!["--cfg", "preferred_bare_config"],
        "Bare 'config' file must take precedence over 'config.toml' in the same directory"
    );
}

#[test]
fn test_compiler_args_fails_closed_on_config_cli_arg() {
    let dummy_unit = PlannedCompilationUnit {
        package_id: "test_pkg 0.1.0".to_string(),
        package_name: "test_pkg".to_string(),
        target_name: "test_pkg".to_string(),
        kind: "lib".to_string(),
        version: "0.1.0".to_string(),
        manifest_path: PathBuf::from("Cargo.toml"),
        src_path: PathBuf::from("src/lib.rs"),
        edition: Some("2021".to_string()),
        cache_eligibility: "eligible".to_string(),
        is_workspace: true,
        is_primary: false,
        env_vars: Vec::new(),
        dependencies: Vec::new(),
        features: Vec::new(),
        for_host: false,
    };

    let inv = RustcInvocation {
        unit: &dummy_unit,
        mode: BuildMode::Build,
        release: false,
        action_key: "test_key",
        out_dir: Path::new("target/out"),
        dest_binary: None,
        deps_dir: None,
        extern_crates: &[],
        features: &[],
        directives: None,
        jobserver_auth: None,
        extra_compiler_args: &["--config".to_string(), "build.rustflags=['-v']".to_string()],
    };

    let res = build_compiler_command(&inv);
    assert!(res.is_err());
    let err_str = res.unwrap_err().to_string();
    assert!(
        err_str.contains("Unsupported option: --config is not yet supported in mitts"),
        "Unexpected error: {}",
        err_str
    );
}
