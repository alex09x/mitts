/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::rust::compiler::RustcInvocation;
use mitts::adapter::rust::compiler_args::build_compiler_command;
use mitts::adapter::rust::key::{derive_unit_action_key_context, derive_unit_action_keys};
use mitts::adapter::rust::key_flags::receives_target_rustflags;
use mitts::adapter::rust::types::{BuildMode, PlannedCompilationUnit, RustBuildOptions};
use mitts::adapter::RustAdapter;
use mitts::core::action_cache::ActionCache;
use std::fs;
use std::path::PathBuf;
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
        std::env::remove_var("CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUSTFLAGS");
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
        std::env::remove_var("CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUSTFLAGS");
    }
}

fn make_dummy_unit(name: &str, kind: &str) -> PlannedCompilationUnit {
    PlannedCompilationUnit {
        package_id: format!("{}@0.1.0", name),
        package_name: name.to_string(),
        version: "0.1.0".to_string(),
        target_name: name.to_string(),
        kind: kind.to_string(),
        src_path: PathBuf::from(format!("src/{}.rs", name)),
        manifest_path: PathBuf::from("Cargo.toml"),
        cache_eligibility: "Cached".to_string(),
        is_workspace: true,
        is_primary: false,
        env_vars: Vec::new(),
        dependencies: Vec::new(),
        edition: Some("2021".to_string()),
        features: Vec::new(),
        for_host: false,
        workspace_root: None,
    }
}

#[test]
fn test_receives_target_rustflags_predicate_unit_matrix() {
    let lib_unit = make_dummy_unit("my_lib", "lib");
    let bin_unit = make_dummy_unit("my_bin", "bin");
    let test_unit = make_dummy_unit("my_test", "test");
    let pm_unit = make_dummy_unit("my_macro", "proc-macro");
    let cb_unit = make_dummy_unit("my_build", "custom-build");
    let bs_unit = make_dummy_unit("my_script", "build-script");

    // Case 1: Native build without explicit --target (explicit_target = None)
    // Cargo semantics: all units compile for host and receive rustflags.
    assert!(receives_target_rustflags(&lib_unit, None));
    assert!(receives_target_rustflags(&bin_unit, None));
    assert!(receives_target_rustflags(&test_unit, None));
    assert!(receives_target_rustflags(&pm_unit, None));
    assert!(receives_target_rustflags(&cb_unit, None));
    assert!(receives_target_rustflags(&bs_unit, None));

    // Case 2: Cross-compilation with explicit target (explicit_target = Some(...))
    // Cargo semantics: host-only units (proc-macro, custom-build, build-script)
    // MUST NOT receive target rustflags. Target units (lib, bin, test) DO receive them.
    let explicit_target = Some("aarch64-unknown-linux-gnu");
    assert!(receives_target_rustflags(&lib_unit, explicit_target));
    assert!(receives_target_rustflags(&bin_unit, explicit_target));
    assert!(receives_target_rustflags(&test_unit, explicit_target));
    assert!(!receives_target_rustflags(&pm_unit, explicit_target));
    assert!(!receives_target_rustflags(&cb_unit, explicit_target));
    assert!(!receives_target_rustflags(&bs_unit, explicit_target));
}

#[test]
fn test_proc_macro_and_target_rustflags_parity_between_key_and_compiler() {
    let _lock = SERIAL_TEST_LOCK.lock().unwrap();
    let temp = TempDir::new().unwrap();
    let root = temp.path().to_path_buf();
    let _guard = EnvGuard::new(&root.join("empty_cargo_home"));

    // Set up workspace with a library member and a proc-macro member
    fs::write(
        root.join("Cargo.toml"),
        r#"
[workspace]
members = ["lib_pkg", "pm_pkg"]
resolver = "2"
"#,
    )
    .unwrap();

    let lib_dir = root.join("lib_pkg");
    fs::create_dir_all(lib_dir.join("src")).unwrap();
    fs::write(
        lib_dir.join("Cargo.toml"),
        r#"
[package]
name = "lib_pkg"
version = "0.1.0"
edition = "2021"

[lib]
path = "src/lib.rs"
"#,
    )
    .unwrap();
    fs::write(
        lib_dir.join("src/lib.rs"),
        "pub fn lib_func() -> u32 { 100 }\n",
    )
    .unwrap();

    let pm_dir = root.join("pm_pkg");
    fs::create_dir_all(pm_dir.join("src")).unwrap();
    fs::write(
        pm_dir.join("Cargo.toml"),
        r#"
[package]
name = "pm_pkg"
version = "0.1.0"
edition = "2021"

[lib]
proc-macro = true
path = "src/lib.rs"
"#,
    )
    .unwrap();
    fs::write(pm_dir.join("src/lib.rs"), "// proc macro dummy\n").unwrap();

    let cargo_dir = root.join(".cargo");
    fs::create_dir_all(&cargo_dir).unwrap();
    let config_file = cargo_dir.join("config.toml");

    let out_dir = root.join("target/debug/deps");

    // =========================================================================
    // Part A: Native build without --target
    // =========================================================================
    fs::write(
        &config_file,
        r#"
[build]
rustflags = ["--cfg", "native_flag_v1"]
"#,
    )
    .unwrap();

    let opt_native = RustBuildOptions {
        manifest_path: Some(root.join("Cargo.toml")),
        start_dir: Some(root.clone()),
        ..Default::default()
    };
    let plan_native = RustAdapter::explain_plan(&opt_native).expect("Plan native failed");

    let pm_unit = plan_native
        .units
        .iter()
        .find(|u| u.package_name == "pm_pkg")
        .expect("pm_pkg missing");
    let lib_unit = plan_native
        .units
        .iter()
        .find(|u| u.package_name == "lib_pkg")
        .expect("lib_pkg missing");

    let keys_native_v1 = derive_unit_action_keys(&opt_native, &plan_native).unwrap();
    let pm_key_nat_v1 = keys_native_v1.get(&pm_unit.unit_id()).unwrap().clone();
    let lib_key_nat_v1 = keys_native_v1.get(&lib_unit.unit_id()).unwrap().clone();

    // Verify compiler command line on native build includes config rustflags
    let inv_pm_nat = RustcInvocation {
        unit: pm_unit,
        action_key: &pm_key_nat_v1,
        out_dir: &out_dir,
        dest_binary: None,
        extern_crates: &[],
        release: false,
        features: &[],
        directives: None,
        mode: BuildMode::Build,
        extra_compiler_args: &opt_native.extra_args,
        jobserver_auth: None,
        deps_dir: None,
        target_dir: None,
    };
    let (cmd_pm_nat, _, _) = build_compiler_command(&inv_pm_nat).unwrap();
    let args_pm_nat: Vec<String> = cmd_pm_nat
        .get_args()
        .map(|a| a.to_string_lossy().to_string())
        .collect();
    assert!(
        args_pm_nat.contains(&"native_flag_v1".to_string()),
        "Native proc-macro must receive config rustflags in rustc command: {:?}",
        args_pm_nat
    );

    // Modify native config rustflags
    fs::write(
        &config_file,
        r#"
[build]
rustflags = ["--cfg", "native_flag_v2"]
"#,
    )
    .unwrap();

    let keys_native_v2 = derive_unit_action_keys(&opt_native, &plan_native).unwrap();
    let pm_key_nat_v2 = keys_native_v2.get(&pm_unit.unit_id()).unwrap().clone();
    let lib_key_nat_v2 = keys_native_v2.get(&lib_unit.unit_id()).unwrap().clone();

    // In native mode: changing config rustflags MUST change BOTH action keys
    assert_ne!(
        pm_key_nat_v1, pm_key_nat_v2,
        "Without --target, changing config rustflags must invalidate proc-macro action key"
    );
    assert_ne!(
        lib_key_nat_v1, lib_key_nat_v2,
        "Without --target, changing config rustflags must invalidate lib action key"
    );

    // =========================================================================
    // Part B: Cross-compilation with explicit --target
    // =========================================================================
    let target_triple = "aarch64-unknown-linux-gnu";
    let extra_args_target = vec!["--target".to_string(), target_triple.to_string()];
    let opt_cross = RustBuildOptions {
        manifest_path: Some(root.join("Cargo.toml")),
        start_dir: Some(root.clone()),
        extra_args: extra_args_target.clone(),
        ..Default::default()
    };
    let plan_cross = RustAdapter::explain_plan(&opt_cross).expect("Plan cross failed");

    let pm_unit_cross = plan_cross
        .units
        .iter()
        .find(|u| u.package_name == "pm_pkg")
        .expect("pm_pkg missing in cross plan");
    let lib_unit_cross = plan_cross
        .units
        .iter()
        .find(|u| u.package_name == "lib_pkg")
        .expect("lib_pkg missing in cross plan");

    // Target config v1
    fs::write(
        &config_file,
        r#"
[build]
rustflags = ["--cfg", "target_flag_v1"]

[target.aarch64-unknown-linux-gnu]
rustflags = ["--cfg", "target_flag_v1"]
"#,
    )
    .unwrap();

    let ctx_cross_v1 = derive_unit_action_key_context(&opt_cross, &plan_cross).unwrap();
    let pm_flags_cross_v1 = ctx_cross_v1
        .flags_by_unit
        .get(&pm_unit_cross.unit_id())
        .expect("pm_unit_cross flags missing")
        .clone();
    let lib_flags_cross_v1 = ctx_cross_v1
        .flags_by_unit
        .get(&lib_unit_cross.unit_id())
        .expect("lib_unit_cross flags missing")
        .clone();

    // Verify unit flags: host proc-macro omits target config flags; target lib includes them
    assert!(
        !pm_flags_cross_v1.iter().any(|f| f.starts_with("CONFIG_RUSTFLAG:")),
        "With explicit --target, host proc-macro unit flags must NOT contain target CONFIG_RUSTFLAG: {:?}",
        pm_flags_cross_v1
    );
    assert!(
        lib_flags_cross_v1.contains(&"CONFIG_RUSTFLAG:target_flag_v1".to_string()),
        "With explicit --target, target lib unit flags MUST contain target CONFIG_RUSTFLAG: {:?}",
        lib_flags_cross_v1
    );

    // Verify compiler command line on cross-compilation:
    // 1. Host proc-macro MUST NOT receive target config rustflags
    let inv_pm_cross_v1 = RustcInvocation {
        unit: pm_unit_cross,
        action_key: "dummy_action_key",
        out_dir: &out_dir,
        dest_binary: None,
        extern_crates: &[],
        release: false,
        features: &[],
        directives: None,
        mode: BuildMode::Build,
        extra_compiler_args: &opt_cross.extra_args,
        jobserver_auth: None,
        deps_dir: None,
        target_dir: None,
    };
    let (cmd_pm_cross_v1, _, _) = build_compiler_command(&inv_pm_cross_v1).unwrap();
    let args_pm_cross_v1: Vec<String> = cmd_pm_cross_v1
        .get_args()
        .map(|a| a.to_string_lossy().to_string())
        .collect();
    assert!(
        !args_pm_cross_v1.contains(&"target_flag_v1".to_string()),
        "With explicit --target, host proc-macro command must NOT contain target config rustflags: {:?}",
        args_pm_cross_v1
    );

    // 2. Target library MUST receive target config rustflags
    let inv_lib_cross_v1 = RustcInvocation {
        unit: lib_unit_cross,
        action_key: "dummy_action_key",
        out_dir: &out_dir,
        dest_binary: None,
        extern_crates: &[],
        release: false,
        features: &[],
        directives: None,
        mode: BuildMode::Build,
        extra_compiler_args: &opt_cross.extra_args,
        jobserver_auth: None,
        deps_dir: None,
        target_dir: None,
    };
    let (cmd_lib_cross_v1, _, _) = build_compiler_command(&inv_lib_cross_v1).unwrap();
    let args_lib_cross_v1: Vec<String> = cmd_lib_cross_v1
        .get_args()
        .map(|a| a.to_string_lossy().to_string())
        .collect();
    assert!(
        args_lib_cross_v1.contains(&"target_flag_v1".to_string()),
        "With explicit --target, target lib command MUST contain target config rustflags: {:?}",
        args_lib_cross_v1
    );

    // Target config v2 (modify target rustflags)
    fs::write(
        &config_file,
        r#"
[build]
rustflags = ["--cfg", "target_flag_v2"]

[target.aarch64-unknown-linux-gnu]
rustflags = ["--cfg", "target_flag_v2"]
"#,
    )
    .unwrap();

    let ctx_cross_v2 = derive_unit_action_key_context(&opt_cross, &plan_cross).unwrap();
    let pm_flags_cross_v2 = ctx_cross_v2
        .flags_by_unit
        .get(&pm_unit_cross.unit_id())
        .expect("pm_unit_cross flags missing in v2")
        .clone();
    let lib_flags_cross_v2 = ctx_cross_v2
        .flags_by_unit
        .get(&lib_unit_cross.unit_id())
        .expect("lib_unit_cross flags missing in v2")
        .clone();

    // Crucial Cargo invariant:
    // With explicit --target: target rustflags modification must NOT affect host proc-macro unit flags!
    assert_eq!(
        pm_flags_cross_v1, pm_flags_cross_v2,
        "With explicit --target, target rustflags modification must NOT alter host proc-macro unit flags"
    );
    assert_ne!(
        lib_flags_cross_v1, lib_flags_cross_v2,
        "With explicit --target, target rustflags modification MUST alter target lib unit flags"
    );

    // Parity verification on dynamic action keys holding inputs constant:
    // Demonstrates that under identical package inputs, target rustflag changes
    // leave the host proc-macro action key strictly unchanged, while invalidating the target library key.
    let pm_key_cross_v1 = ActionCache::derive_action_key(
        &ctx_cross_v1.toolchain_version,
        &ctx_cross_v1.target_triple,
        &ctx_cross_v1.profile,
        &pm_flags_cross_v1,
        &pm_unit_cross.features,
        &[],
        &[],
    );
    let pm_key_cross_v2 = ActionCache::derive_action_key(
        &ctx_cross_v2.toolchain_version,
        &ctx_cross_v2.target_triple,
        &ctx_cross_v2.profile,
        &pm_flags_cross_v2,
        &pm_unit_cross.features,
        &[],
        &[],
    );
    assert_eq!(
        pm_key_cross_v1, pm_key_cross_v2,
        "With explicit --target, target rustflags change must NOT alter host proc-macro action key"
    );

    let lib_key_cross_v1 = ActionCache::derive_action_key(
        &ctx_cross_v1.toolchain_version,
        &ctx_cross_v1.target_triple,
        &ctx_cross_v1.profile,
        &lib_flags_cross_v1,
        &lib_unit_cross.features,
        &[],
        &[],
    );
    let lib_key_cross_v2 = ActionCache::derive_action_key(
        &ctx_cross_v2.toolchain_version,
        &ctx_cross_v2.target_triple,
        &ctx_cross_v2.profile,
        &lib_flags_cross_v2,
        &lib_unit_cross.features,
        &[],
        &[],
    );
    assert_ne!(
        lib_key_cross_v1, lib_key_cross_v2,
        "With explicit --target, target rustflags change MUST alter target lib action key"
    );

    // Verify compiler command line on v2:
    let inv_pm_cross_v2 = RustcInvocation {
        unit: pm_unit_cross,
        action_key: "dummy_action_key",
        out_dir: &out_dir,
        dest_binary: None,
        extern_crates: &[],
        release: false,
        features: &[],
        directives: None,
        mode: BuildMode::Build,
        extra_compiler_args: &opt_cross.extra_args,
        jobserver_auth: None,
        deps_dir: None,
        target_dir: None,
    };
    let (cmd_pm_cross_v2, _, _) = build_compiler_command(&inv_pm_cross_v2).unwrap();
    let args_pm_cross_v2: Vec<String> = cmd_pm_cross_v2
        .get_args()
        .map(|a| a.to_string_lossy().to_string())
        .collect();
    assert!(
        !args_pm_cross_v2.contains(&"target_flag_v2".to_string()),
        "With explicit --target, host proc-macro command must NOT contain v2 target config rustflags"
    );

    let inv_lib_cross_v2 = RustcInvocation {
        unit: lib_unit_cross,
        action_key: "dummy_action_key",
        out_dir: &out_dir,
        dest_binary: None,
        extern_crates: &[],
        release: false,
        features: &[],
        directives: None,
        mode: BuildMode::Build,
        extra_compiler_args: &opt_cross.extra_args,
        jobserver_auth: None,
        deps_dir: None,
        target_dir: None,
    };
    let (cmd_lib_cross_v2, _, _) = build_compiler_command(&inv_lib_cross_v2).unwrap();
    let args_lib_cross_v2: Vec<String> = cmd_lib_cross_v2
        .get_args()
        .map(|a| a.to_string_lossy().to_string())
        .collect();
    assert!(
        args_lib_cross_v2.contains(&"target_flag_v2".to_string()),
        "With explicit --target, target lib command MUST contain v2 target config rustflags"
    );
}
