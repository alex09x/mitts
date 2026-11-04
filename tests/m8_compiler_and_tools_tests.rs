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
use mitts::adapter::rust::types::{BuildMode, PlannedCompilationUnit, RustBuildOptions};
use mitts::adapter::rust::RustAdapter;
use std::fs;

#[test]
fn test_m8_rust_adapter_resolve_manifest_and_target_dir() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let manifest_file = temp_dir.path().join("Cargo.toml");
    fs::write(
        &manifest_file,
        "[package]\nname = \"dummy\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();

    // 1. Explicit manifest path that exists
    let opt = RustBuildOptions {
        manifest_path: Some(manifest_file.clone()),
        ..RustBuildOptions::default()
    };
    let resolved = RustAdapter::resolve_manifest(&opt).expect("resolve manifest");
    assert_eq!(resolved, manifest_file);

    // 2. Explicit manifest that does not exist
    let opt_missing = RustBuildOptions {
        manifest_path: Some(temp_dir.path().join("non_existent_Cargo.toml")),
        ..RustBuildOptions::default()
    };
    assert!(RustAdapter::resolve_manifest(&opt_missing).is_err());

    // 3. Resolve target dir explicit
    let custom_target = temp_dir.path().join("custom_target");
    let opt_target = RustBuildOptions {
        target_dir: Some(custom_target.clone()),
        ..RustBuildOptions::default()
    };
    let res_target = RustAdapter::resolve_target_dir(&manifest_file, &opt_target).unwrap();
    assert_eq!(res_target, custom_target);

    // 4. Resolve target dir default (target)
    let res_default = RustAdapter::resolve_target_dir(&manifest_file, &opt).unwrap();
    assert_eq!(res_default, temp_dir.path().join("target"));
}

#[test]
fn test_target_root_and_test_env_agree() {
    use mitts::adapter::rust::unit_env::{apply_test_binary_env, compute_unit_rustc_env_with_target};
    use std::path::PathBuf;
    use std::process::Command;

    let Ok(case) = std::env::var("MITTS_TARGET_LAYOUT_PROBE") else {
        for case in ["default", "env-relative", "env-absolute", "explicit"] {
            let cwd = tempfile::tempdir().unwrap();
            let mut cmd = Command::new(std::env::current_exe().unwrap());
            cmd.args([
                "--exact",
                "test_target_root_and_test_env_agree",
                "--nocapture",
            ])
            .current_dir(cwd.path())
            .env("MITTS_TARGET_LAYOUT_PROBE", case)
            .env_remove("CARGO_TARGET_DIR");
            match case {
                "env-relative" => {
                    cmd.env("CARGO_TARGET_DIR", "env-parent/out");
                }
                "env-absolute" | "explicit" => {
                    cmd.env("CARGO_TARGET_DIR", cwd.path().join("env-parent/out"));
                }
                _ => {}
            }
            let out = cmd.output().unwrap();
            assert!(
                out.status.success(),
                "{case}: {}\n{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
        }
        return;
    };

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let member = root.join("member");
    fs::create_dir_all(member.join("src")).unwrap();
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"member\"]\n",
    )
    .unwrap();
    fs::write(
        member.join("Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(member.join("src/main.rs"), "fn main() {}\n").unwrap();
    let explicit = (case == "explicit").then(|| PathBuf::from("custom/target/mitts"));
    let opt = RustBuildOptions {
        target_dir: explicit.clone(),
        ..Default::default()
    };
    let target = RustAdapter::resolve_target_dir(&root.join("Cargo.toml"), &opt).unwrap();
    let expected = explicit
        .clone()
        .or_else(|| std::env::var_os("CARGO_TARGET_DIR").map(PathBuf::from))
        .unwrap_or_else(|| root.join("target"));
    assert_eq!(target, expected);
    if explicit.is_none() {
        assert!(target.join("CACHEDIR.TAG").is_file());
        assert!(target.join(".gitignore").is_file());
        assert!(!target.parent().unwrap().join("CACHEDIR.TAG").exists());
        assert!(!target.parent().unwrap().join(".gitignore").exists());
    }

    let unit = PlannedCompilationUnit {
        package_name: "app".into(),
        version: "0.1.0".into(),
        kind: "test".into(),
        manifest_path: member.join("Cargo.toml"),
        workspace_root: Some(root.to_path_buf()),
        ..Default::default()
    };
    for release in [false, true] {
        for target_override in [explicit.as_deref(), Some(target.as_path())] {
            let compile_env =
                compute_unit_rustc_env_with_target(&unit, None, None, target_override, release);
            let mut cmd = Command::new("unused");
            apply_test_binary_env(&mut cmd, &unit, &member, target_override, release);
            let runtime_env: std::collections::HashMap<_, _> = cmd
                .get_envs()
                .map(|(key, value)| {
                    (
                        key.to_string_lossy().to_string(),
                        value.unwrap().to_string_lossy().to_string(),
                    )
                })
                .collect();
            for key in ["CARGO_BIN_EXE_app", "CARGO_TARGET_TMPDIR"] {
                assert_eq!(compile_env[key], runtime_env[key]);
            }
            let profile = if release { "release" } else { "debug" };
            assert_eq!(
                PathBuf::from(&compile_env["CARGO_BIN_EXE_app"]),
                target.join(profile).join("app")
            );
            assert_eq!(
                PathBuf::from(&compile_env["CARGO_TARGET_TMPDIR"]),
                target.join("tmp")
            );
        }
    }
}

#[test]
fn test_m8_rust_adapter_detect_toolchain_and_cache() {
    // Should detect toolchain successfully and write/read toolchain cache
    let info = RustAdapter::detect_toolchain().expect("detect toolchain");
    assert_eq!(info.name, "rustc");
    assert!(!info.version.is_empty());

    // Call second time to hit cache
    let cached_info = RustAdapter::detect_toolchain().expect("detect toolchain cached");
    assert_eq!(info.version, cached_info.version);
    assert_eq!(info.target_triple, cached_info.target_triple);
}

#[test]
fn test_m8_compiler_invocation_modes_and_args() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let out_dir = temp_dir.path().join("out");
    fs::create_dir_all(&out_dir).unwrap();

    let src_file = temp_dir.path().join("lib.rs");
    fs::write(&src_file, "pub fn value() -> u32 { 100 }\n").unwrap();

    // Test with pre-release version semver and extra compiler args
    let unit = PlannedCompilationUnit {
        package_id: "demo-pkg 1.2.3-beta.1".to_string(),
        package_name: "demo-pkg".to_string(),
        version: "1.2.3-beta.1".to_string(),
        target_name: "demo_pkg".to_string(),
        kind: "lib".to_string(),
        src_path: src_file,
        manifest_path: temp_dir.path().join("Cargo.toml"),
        cache_eligibility: "eligible".to_string(),
        is_workspace: true,
        is_primary: false,
        env_vars: vec![],
        dependencies: vec![],
        edition: Some("2021".to_string()),
        features: vec![],
        for_host: false,
        workspace_root: None,
    };

    let extra_args = vec!["-C".to_string(), "opt-level=0".to_string()];
    let inv = RustcInvocation {
        unit: &unit,
        action_key: "short_key_123",
        out_dir: &out_dir,
        dest_binary: None,
        extern_crates: &[],
        release: false,
        features: &[],
        directives: None,
        mode: BuildMode::Build,
        extra_compiler_args: &extra_args,
        jobserver_auth: None,
        deps_dir: None,
        target_dir: None,
        cache_root: None,
    };

    let out = inv.execute().expect("execute rustc");
    assert!(out.success, "rustc execution failed: {}", out.stderr);
    assert!(out.primary_output.is_some());

    // Test check mode
    let inv_check = RustcInvocation {
        unit: &unit,
        action_key: "short_key_123",
        out_dir: &out_dir,
        dest_binary: None,
        extern_crates: &[],
        release: false,
        features: &[],
        directives: None,
        mode: BuildMode::Check,
        extra_compiler_args: &[],
        jobserver_auth: None,
        deps_dir: None,
        target_dir: None,
        cache_root: None,
    };

    let out_check = inv_check.execute().expect("execute check");
    assert!(out_check.success);
}

#[test]
fn test_m8_compiler_non_workspace_cap_lints() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let out_dir = temp_dir.path().join("out_non_ws");
    fs::create_dir_all(&out_dir).unwrap();

    let src_file = temp_dir.path().join("lib.rs");
    // Code with unused variable that would trigger a warning
    fs::write(&src_file, "pub fn foo() { let x = 1; }\n").unwrap();

    let unit = PlannedCompilationUnit {
        package_id: "ext-pkg 0.1.0".to_string(),
        package_name: "ext-pkg".to_string(),
        version: "0.1.0".to_string(),
        target_name: "ext_pkg".to_string(),
        kind: "lib".to_string(),
        src_path: src_file,
        manifest_path: temp_dir.path().join("Cargo.toml"),
        cache_eligibility: "eligible".to_string(),
        is_workspace: false, // Non-workspace crate -> cap-lints allow
        is_primary: false,
        env_vars: vec![],
        dependencies: vec![],
        edition: Some("2021".to_string()),
        features: vec![],
        for_host: false,
        workspace_root: None,
    };

    let inv = RustcInvocation {
        unit: &unit,
        action_key: "non_ws_key_456",
        out_dir: &out_dir,
        dest_binary: None,
        extern_crates: &[],
        release: false,
        features: &[],
        directives: None,
        mode: BuildMode::Build,
        extra_compiler_args: &[],
        jobserver_auth: None,
        deps_dir: None,
        target_dir: None,
        cache_root: None,
    };

    let out = inv.execute().expect("execute rustc non workspace");
    assert!(out.success);
}
