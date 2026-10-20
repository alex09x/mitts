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
use mitts::adapter::rust::dep_info::parse_dep_info_full;
use mitts::adapter::rust::inputs::is_immutable_package_dir;
use mitts::adapter::rust::key_flags::compute_unit_flags_with_ws;
use mitts::adapter::rust::profile::resolve_unit_profile;
use mitts::adapter::rust::types::{
    BuildMode, PlannedCompilationUnit, RustAdapterError, RustBuildOptions,
};
use mitts::adapter::rust::unit_inputs::{
    compute_package_cache_entry, compute_unit_coarse_inputs_with_checksum,
    format_immutable_pkg_label,
};
use mitts::adapter::RustAdapter;
use mitts::core::action_manifest::{ActionManifest, ActionManifestEntry};
use mitts::core::build_lock::{FastBuildLock, RecordStateOptions};
use mitts::core::build_lock_types::{FreshnessCheckArgs, FreshnessResult, RecordStateArgs};
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use tempfile::tempdir;

static ENV_MUTEX: Mutex<()> = Mutex::new(());

fn base_options(manifest_path: PathBuf, target_dir: PathBuf) -> RustBuildOptions {
    RustBuildOptions {
        manifest_path: Some(manifest_path.clone()),
        target_dir: Some(target_dir),
        start_dir: manifest_path.parent().map(|p| p.to_path_buf()),
        ..Default::default()
    }
}

#[test]
fn test_b1_fast_lock_tracks_non_rs_include_str_and_option_env() {
    let _guard = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempdir().unwrap();
    let cache_dir = tempdir().unwrap();
    // SAFETY: serialized via ENV_MUTEX in test process.
    unsafe {
        std::env::set_var("MITTS_CACHE_DIR", cache_dir.path());
        std::env::set_var("CARGO_HOME", dir.path().join(".cargo-home"));
        std::env::set_var("MITTS_B1_ENV_VAR", "env_val_1");
    }

    let manifest = dir.path().join("Cargo.toml");
    fs::write(
        &manifest,
        r#"[package]
name = "b1_app"
version = "0.1.0"
edition = "2021"
"#,
    )
    .unwrap();
    let src_dir = dir.path().join("src");
    fs::create_dir_all(&src_dir).unwrap();
    fs::write(src_dir.join("data.json"), r#"{"msg":"json_v1"}"#).unwrap();
    fs::write(
        src_dir.join("main.rs"),
        r#"fn main() {
    let data = include_str!("data.json");
    let env_v = option_env!("MITTS_B1_ENV_VAR").unwrap_or("unset");
    println!("{data}|{env_v}");
}
"#,
    )
    .unwrap();

    let target_dir = dir.path().join("target");
    let opt = base_options(manifest.clone(), target_dir.clone());
    RustAdapter::build(&opt).unwrap();

    let bin_path = target_dir.join("debug").join("b1_app");
    let out1 = std::process::Command::new(&bin_path).output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out1.stdout).trim(),
        r#"{"msg":"json_v1"}|env_val_1"#
    );

    // Edit non-.rs file included via include_str! -> must NOT hit fast_lock and must update binary.
    fs::write(src_dir.join("data.json"), r#"{"msg":"json_v2_updated"}"#).unwrap();
    RustAdapter::build(&opt).unwrap();
    let out2 = std::process::Command::new(&bin_path).output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out2.stdout).trim(),
        r#"{"msg":"json_v2_updated"}|env_val_1"#
    );

    // Change option_env! variable -> must NOT hit fast_lock and must update binary.
    unsafe {
        std::env::set_var("MITTS_B1_ENV_VAR", "env_val_2_updated");
    }
    RustAdapter::build(&opt).unwrap();
    let out3 = std::process::Command::new(&bin_path).output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out3.stdout).trim(),
        r#"{"msg":"json_v2_updated"}|env_val_2_updated"#
    );
    unsafe {
        std::env::remove_var("MITTS_B1_ENV_VAR");
    }
}

#[test]
fn test_b1_external_path_dep_and_incomplete_inputs_fail_closed() {
    let _guard = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    let root = tempdir().unwrap();
    let cache_dir = tempdir().unwrap();
    unsafe {
        std::env::set_var("MITTS_CACHE_DIR", cache_dir.path());
        std::env::set_var("CARGO_HOME", root.path().join(".cargo-home"));
    }

    let ext_dir = root.path().join("ext_dep");
    fs::create_dir_all(ext_dir.join("src")).unwrap();
    fs::write(
        ext_dir.join("Cargo.toml"),
        "[package]\nname = \"ext_dep\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(ext_dir.join("src/msg.txt"), "ext_v1").unwrap();
    fs::write(
        ext_dir.join("src/lib.rs"),
        "pub fn msg() -> &'static str { include_str!(\"msg.txt\") }\n",
    )
    .unwrap();

    let app_dir = root.path().join("app");
    fs::create_dir_all(app_dir.join("src")).unwrap();
    fs::write(
        app_dir.join("Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[dependencies]\next_dep = { path = \"../ext_dep\" }\n",
    )
    .unwrap();
    fs::write(
        app_dir.join("src/main.rs"),
        "fn main() { println!(\"{}\", ext_dep::msg()); }\n",
    )
    .unwrap();

    let target_dir = app_dir.join("target");
    let opt = base_options(app_dir.join("Cargo.toml"), target_dir.clone());
    RustAdapter::build(&opt).unwrap();

    let bin_path = target_dir.join("debug").join("app");
    let out1 = std::process::Command::new(&bin_path).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out1.stdout).trim(), "ext_v1");

    // Edit non-.rs file in external path dependency outside project_dir.
    fs::write(ext_dir.join("src/msg.txt"), "ext_v2_modified").unwrap();
    RustAdapter::build(&opt).unwrap();
    let out2 = std::process::Command::new(&bin_path).output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out2.stdout).trim(),
        "ext_v2_modified"
    );

    // Verify FastBuildLock fails closed when has_complete_inputs is false.
    let incomplete_dir = tempdir().unwrap();
    fs::write(
        incomplete_dir.path().join("Cargo.toml"),
        "[package]\nname = \"inc\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    let inc_target = incomplete_dir.path().join("target");
    let none_pkg = None;
    FastBuildLock::record_state_with_options(
        &RecordStateArgs {
            project_dir: incomplete_dir.path(),
            profile: "dev",
            package: &none_pkg,
            features: &[],
            no_default_features: false,
            all_features: false,
            toolchain_version: "rustc 1.80.0",
            action_count: 1,
            root_binary: None,
            outputs: &[],
            stderr: None,
        },
        &RecordStateOptions {
            target_dir: Some(&inc_target),
            options_hash: Some("opt_hash"),
            extra_package_dirs: &[],
            tracked_extra_files: None,
            tracked_env_vars: None,
            has_complete_inputs: false,
            ..Default::default()
        },
    )
    .unwrap();
    let freshness = FastBuildLock::check_freshness_with_options(
        &FreshnessCheckArgs {
            project_dir: incomplete_dir.path(),
            profile: "dev",
            package: &none_pkg,
            features: &[],
            no_default_features: false,
            all_features: false,
            toolchain_version: "rustc 1.80.0",
        },
        Some(&inc_target),
        Some("opt_hash"),
        true,
    );
    assert!(
        matches!(freshness, FreshnessResult::Stale(ref reason) if reason.contains("Incomplete tracked inputs")),
        "Incomplete inputs must fail closed as Stale, got {:?}",
        freshness
    );
}

#[test]
fn test_b2_fast_lock_options_hash_and_custom_target_dir() {
    let _guard = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempdir().unwrap();
    let cache_dir = tempdir().unwrap();
    unsafe {
        std::env::set_var("MITTS_CACHE_DIR", cache_dir.path());
        std::env::set_var("CARGO_HOME", dir.path().join(".cargo-home"));
    }

    let manifest = dir.path().join("Cargo.toml");
    fs::write(
        &manifest,
        r#"[package]
name = "b2_multi"
version = "0.1.0"
edition = "2021"
"#,
    )
    .unwrap();
    fs::create_dir_all(dir.path().join("src/bin")).unwrap();
    fs::write(dir.path().join("src/bin/a.rs"), "fn main() {}\n").unwrap();
    fs::write(dir.path().join("src/bin/b.rs"), "fn main() {}\n").unwrap();

    let custom_target = dir.path().join("custom_target_dir");
    let mut opt_a = base_options(manifest.clone(), custom_target.clone());
    opt_a.bin_targets = vec!["a".to_string()];
    RustAdapter::build(&opt_a).unwrap();

    assert!(custom_target.join("debug/a").is_file());
    assert!(!custom_target.join("debug/b").is_file());
    assert!(
        FastBuildLock::lock_path_in_target(&custom_target, "dev").is_file(),
        "Lockfile must be written inside custom target_dir"
    );

    // Building --bin b immediately after --bin a must NOT falsely hit fast_lock.
    let mut opt_b = base_options(manifest, custom_target.clone());
    opt_b.bin_targets = vec!["b".to_string()];
    RustAdapter::build(&opt_b).unwrap();
    assert!(
        custom_target.join("debug/b").is_file(),
        "--bin b must be built instead of falsely returning fast_lock=1 from --bin a"
    );
}

#[test]
fn test_b3_immutable_pkg_detection_and_package_id_checksum_disambiguation() {
    let _guard = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempdir().unwrap();
    let cargo_home = dir.path().join(".cargo-home");
    unsafe {
        std::env::set_var("CARGO_HOME", &cargo_home);
    }

    // (a) Vendored crate with .cargo-ok in vendor/foo or local git/checkouts path outside CARGO_HOME must be mutable.
    let vendor_foo = dir.path().join("vendor").join("foo");
    fs::create_dir_all(vendor_foo.join("src")).unwrap();
    fs::write(
        vendor_foo.join("Cargo.toml"),
        "[package]\nname = \"foo\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(vendor_foo.join(".cargo-ok"), "{}").unwrap();
    fs::write(vendor_foo.join("src/lib.rs"), "pub fn v() -> u32 { 1 }\n").unwrap();

    assert!(
        !is_immutable_package_dir(&vendor_foo, false),
        "vendor/foo with .cargo-ok must NOT be treated as an immutable global cache dir"
    );
    let fake_git_local = dir.path().join("local/git/checkouts/foo");
    fs::create_dir_all(&fake_git_local).unwrap();
    assert!(
        !is_immutable_package_dir(&fake_git_local, false),
        "Path ending in git/checkouts outside CARGO_HOME must NOT be treated as immutable"
    );

    let unit_vendor = PlannedCompilationUnit {
        package_id: "foo@0.1.0".to_string(),
        package_name: "foo".to_string(),
        version: "0.1.0".to_string(),
        target_name: "foo".to_string(),
        kind: "lib".to_string(),
        src_path: vendor_foo.join("src/lib.rs"),
        manifest_path: vendor_foo.join("Cargo.toml"),
        is_workspace: false,
        ..Default::default()
    };
    let entry1 = compute_package_cache_entry(&vendor_foo, None, false).unwrap();
    let inputs1 =
        compute_unit_coarse_inputs_with_checksum(&unit_vendor, &vendor_foo, &entry1, None).unwrap();
    assert!(!inputs1.iter().any(|(k, _)| k == "immutable_pkg"));

    fs::write(vendor_foo.join("src/lib.rs"), "pub fn v() -> u32 { 2 }\n").unwrap();
    let entry2 = compute_package_cache_entry(&vendor_foo, None, false).unwrap();
    let inputs2 =
        compute_unit_coarse_inputs_with_checksum(&unit_vendor, &vendor_foo, &entry2, None).unwrap();
    assert_ne!(
        inputs1, inputs2,
        "Editing vendor/foo/src/lib.rs must invalidate unit inputs even when .cargo-ok exists"
    );

    // (b) Multi-crate git checkout under $CARGO_HOME/git/checkouts/repo-xxxx/<rev>/crates/foo with .cargo-ok
    let checkout_root = cargo_home.join("git/checkouts/repo-1234/abcdef1");
    let git_crate = checkout_root.join("crates/foo");
    fs::create_dir_all(git_crate.join("src")).unwrap();
    fs::write(checkout_root.join(".cargo-ok"), "").unwrap();
    fs::write(
        git_crate.join("Cargo.toml"),
        "[package]\nname = \"foo\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(git_crate.join("src/lib.rs"), "").unwrap();
    assert!(is_immutable_package_dir(&git_crate, false));

    let mut unit_git_a = unit_vendor.clone();
    unit_git_a.manifest_path = git_crate.join("Cargo.toml");
    unit_git_a.src_path = git_crate.join("src/lib.rs");
    unit_git_a.package_id = "foo@0.1.0 (git+https://example.com/repo#abcdef1)".to_string();
    let mut unit_git_b = unit_git_a.clone();
    unit_git_b.package_id = "foo@0.1.0 (git+https://example.com/repo#9999999)".to_string();

    let git_entry = compute_package_cache_entry(&git_crate, None, false).unwrap();
    let git_in_a =
        compute_unit_coarse_inputs_with_checksum(&unit_git_a, &git_crate, &git_entry, None)
            .unwrap();
    let git_in_b =
        compute_unit_coarse_inputs_with_checksum(&unit_git_b, &git_crate, &git_entry, None)
            .unwrap();
    assert_ne!(
        git_in_a, git_in_b,
        "Updating #rev in multi-crate git checkout must change immutable_pkg input"
    );

    // (c) Same crate name/version with different checksums or sources produces distinct labels.
    let lbl1 = format_immutable_pkg_label(&unit_git_a, Some("cksum_aaa"));
    let lbl2 = format_immutable_pkg_label(&unit_git_a, Some("cksum_bbb"));
    assert_ne!(lbl1, lbl2);
    assert!(lbl1.contains(":cksum:cksum_aaa"));
}

#[test]
fn test_b4_profile_and_cargo_config_validation_and_flag_propagation() {
    let _guard = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempdir().unwrap();
    unsafe {
        std::env::set_var("CARGO_HOME", dir.path().join(".cargo-home"));
    }

    // 1. Unsupported profile key must return RustAdapterError::UnsupportedOption naming key & file.
    let manifest = dir.path().join("Cargo.toml");
    fs::write(
        &manifest,
        r#"[package]
name = "b4_pkg"
version = "0.1.0"
edition = "2021"

[profile.dev]
split-debuginfo = "packed"
"#,
    )
    .unwrap();
    let err = resolve_unit_profile(&manifest, "b4_pkg", true, false, false, Some(dir.path()))
        .unwrap_err();
    let adapter_err = err
        .downcast_ref::<RustAdapterError>()
        .expect("Expected RustAdapterError");
    assert!(
        matches!(adapter_err, RustAdapterError::UnsupportedOption(msg)
            if msg.contains("profile.dev.split-debuginfo") && msg.contains("Cargo.toml")),
        "Unexpected error: {:?}",
        adapter_err
    );

    // 2. Unsupported .cargo/config.toml [env] must return RustAdapterError::UnsupportedOption.
    fs::write(
        &manifest,
        "[package]\nname = \"b4_pkg\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    let cargo_dir = dir.path().join(".cargo");
    fs::create_dir_all(&cargo_dir).unwrap();
    let cfg_file = cargo_dir.join("config.toml");
    fs::write(&cfg_file, "[env]\nCUSTOM_CFG_VAR = \"1\"\n").unwrap();
    let cfg_err = mitts::adapter::rust::cargo_config::load_cargo_config_rustflags(dir.path(), None)
        .unwrap_err();
    let cfg_adapter_err = cfg_err
        .downcast_ref::<RustAdapterError>()
        .expect("Expected RustAdapterError");
    assert!(
        matches!(cfg_adapter_err, RustAdapterError::UnsupportedOption(msg)
            if msg.contains("env.CUSTOM_CFG_VAR") && msg.contains("config.toml")),
        "Unexpected error: {:?}",
        cfg_adapter_err
    );
    fs::remove_file(&cfg_file).unwrap();

    // 3. Supported profile keys propagate to both compiler_args and key_flags using Cargo rules.
    fs::write(
        &manifest,
        r#"[package]
name = "b4_pkg"
version = "0.1.0"
edition = "2021"

[profile.dev]
opt-level = 2
panic = "abort"
codegen-units = 4
lto = "thin"
strip = "symbols"
overflow-checks = false
debug-assertions = true
incremental = true
debug = 1
"#,
    )
    .unwrap();
    let src_dir = dir.path().join("src");
    fs::create_dir_all(&src_dir).unwrap();
    fs::write(src_dir.join("main.rs"), "fn main() {}\n").unwrap();

    let unit = PlannedCompilationUnit {
        package_id: "b4_pkg@0.1.0".to_string(),
        package_name: "b4_pkg".to_string(),
        version: "0.1.0".to_string(),
        target_name: "b4_pkg".to_string(),
        kind: "bin".to_string(),
        src_path: src_dir.join("main.rs"),
        manifest_path: manifest.clone(),
        is_workspace: true,
        workspace_root: Some(dir.path().to_path_buf()),
        ..Default::default()
    };
    let flags = compute_unit_flags_with_ws(
        &unit,
        dir.path(),
        &[],
        BuildMode::Build,
        false,
        Some(dir.path()),
    )
    .unwrap();
    for expected in [
        "SCHEMA_VERSION=rust_key_v3",
        "DEBUGINFO=1",
        "OPT_LEVEL=2",
        "PANIC=abort",
        "CODEGEN_UNITS=4",
        "LTO=thin",
        "STRIP=symbols",
        "OVERFLOW_CHECKS=false",
        "DEBUG_ASSERTIONS=true",
    ] {
        assert!(
            flags.iter().any(|f| f == expected),
            "Missing {} in key flags: {:?}",
            expected,
            flags
        );
    }

    let out_dir = dir.path().join("out");
    let inv = RustcInvocation {
        unit: &unit,
        out_dir: &out_dir,
        deps_dir: None,
        extern_crates: &[],
        features: &[],
        directives: None,
        action_key: "0123456789abcdef0123456789abcdef",
        mode: BuildMode::Build,
        release: false,
        extra_compiler_args: &[],
        dest_binary: None,
        target_dir: None,
        cache_root: None,
        jobserver_auth: None,
    };
    let (cmd, _, _) = build_compiler_command(&inv).unwrap();
    let args: Vec<String> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().to_string())
        .collect();
    for expected in [
        "opt-level=2",
        "debuginfo=1",
        "panic=abort",
        "codegen-units=4",
        "lto=thin",
        "strip=symbols",
        "debug-assertions=on",
        "overflow-checks=off",
    ] {
        assert!(
            args.iter().any(|a| a == expected),
            "Missing -C {} in compiler args: {:?}",
            expected,
            args
        );
    }
    assert!(
        !args.iter().any(|a| a == "embed-bitcode=no"),
        "embed-bitcode=no must be omitted when LTO is enabled"
    );
}

#[test]
fn test_r1_r2_r3_profile_defaults_test_mode_panic_and_incomplete_inputs_lock() {
    let _guard = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempdir().unwrap();
    unsafe {
        std::env::set_var("CARGO_HOME", dir.path().join(".cargo-home"));
        std::env::set_var("MITTS_CACHE_DIR", dir.path().join(".mitts-cache"));
    }

    // R1(a): [profile.dev] opt-level = 1 keeps cfg!(debug_assertions) == true
    let r1a_dir = dir.path().join("r1a");
    fs::create_dir_all(r1a_dir.join("src")).unwrap();
    fs::write(
        r1a_dir.join("Cargo.toml"),
        r#"[package]
name = "r1a"
version = "0.1.0"
edition = "2021"

[profile.dev]
opt-level = 1
"#,
    )
    .unwrap();
    fs::write(
        r1a_dir.join("src/main.rs"),
        "fn main() { print!(\"{}\", cfg!(debug_assertions)); }\n",
    )
    .unwrap();
    let opt_r1a = base_options(r1a_dir.join("Cargo.toml"), r1a_dir.join("target"));
    RustAdapter::build(&opt_r1a).unwrap();
    let out_r1a = std::process::Command::new(r1a_dir.join("target/debug/r1a"))
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&out_r1a.stdout), "true");

    // R1(b): release build with build.rs compiles build.rs with cfg!(debug_assertions) == false
    let r1b_dir = dir.path().join("r1b");
    fs::create_dir_all(r1b_dir.join("src")).unwrap();
    fs::write(
        r1b_dir.join("Cargo.toml"),
        "[package]\nname = \"r1b\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(
        r1b_dir.join("build.rs"),
        r#"fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let out = std::env::var("OUT_DIR").unwrap();
    std::fs::write(format!("{}/host_da.txt", out), format!("{}", cfg!(debug_assertions))).unwrap();
}
"#,
    )
    .unwrap();
    fs::write(
        r1b_dir.join("src/main.rs"),
        "fn main() { print!(\"{}\", include_str!(concat!(env!(\"OUT_DIR\"), \"/host_da.txt\"))); }\n",
    )
    .unwrap();
    let mut opt_r1b = base_options(r1b_dir.join("Cargo.toml"), r1b_dir.join("target"));
    opt_r1b.release = true;
    RustAdapter::build(&opt_r1b).unwrap();
    let out_r1b = std::process::Command::new(r1b_dir.join("target/release/r1b"))
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&out_r1b.stdout), "false");

    // R2: [profile.dev] panic = "abort" must not pass -C panic=abort in BuildMode::Test
    let r2_dir = dir.path().join("r2");
    fs::create_dir_all(r2_dir.join("src")).unwrap();
    fs::write(
        r2_dir.join("Cargo.toml"),
        r#"[package]
name = "r2"
version = "0.1.0"
edition = "2021"

[profile.dev]
panic = "abort"
"#,
    )
    .unwrap();
    fs::write(
        r2_dir.join("src/lib.rs"),
        "pub fn add(a: i32, b: i32) -> i32 { a + b }\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn it_works() { assert_eq!(super::add(2, 2), 4); }\n}\n",
    )
    .unwrap();
    let mut opt_r2 = base_options(r2_dir.join("Cargo.toml"), r2_dir.join("target"));
    opt_r2.mode = BuildMode::Test;
    let (plan_r2, res_r2) = RustAdapter::build(&opt_r2).unwrap();
    assert!(
        res_r2.success,
        "BuildMode::Test with panic=abort must succeed"
    );
    let test_bin = plan_r2
        .actions
        .iter()
        .filter(|a| a.kind == mitts::core::action::ActionKind::CompileBin)
        .flat_map(|a| &a.outputs)
        .next()
        .expect("Expected compiled test binary");
    let test_out = std::process::Command::new(test_bin).output().unwrap();
    assert!(test_out.status.success(), "Compiled test binary must pass");

    // R3: missing .d or untracked build script marks has_complete_inputs = false in build.lock
    let empty_stage = dir.path().join("empty_stage");
    fs::create_dir_all(&empty_stage).unwrap();
    let unit_r3 = PlannedCompilationUnit {
        package_id: "r1a@0.1.0".to_string(),
        package_name: "r1a".to_string(),
        version: "0.1.0".to_string(),
        target_name: "r1a".to_string(),
        kind: "bin".to_string(),
        src_path: r1a_dir.join("src/main.rs"),
        manifest_path: r1a_dir.join("Cargo.toml"),
        is_workspace: true,
        ..Default::default()
    };
    assert!(
        mitts::adapter::rust::dag_unit_publish::extract_unit_extra_inputs(
            &unit_r3,
            &empty_stage,
            Some(&r1a_dir),
            None,
            None,
        )
        .is_err(),
        "extract_unit_extra_inputs must fail when staging_dir has no .d file"
    );

    let r3_dir = dir.path().join("r3");
    fs::create_dir_all(r3_dir.join("src")).unwrap();
    fs::write(
        r3_dir.join("Cargo.toml"),
        "[package]\nname = \"r3\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(
        r3_dir.join("src/main.rs"),
        "macro_rules! k { () => { \"DYN_KEY\" }; }\nfn main() { let _ = option_env!(k!()); }\n",
    )
    .unwrap();
    let opt_r3 = base_options(r3_dir.join("Cargo.toml"), r3_dir.join("target"));
    let (plan_r3, _) = RustAdapter::build(&opt_r3).unwrap();
    assert!(
        plan_r3.inputs_incomplete,
        "BuildPlan must mark inputs_incomplete = true when unit inputs are incomplete"
    );
    let lock_bytes = fs::read(FastBuildLock::lock_path_in_target(
        &r3_dir.join("target"),
        "dev",
    ))
    .unwrap();
    let lock: FastBuildLock = serde_json::from_slice(&lock_bytes).unwrap();
    assert!(
        !lock.has_complete_inputs,
        "FastBuildLock must record has_complete_inputs = false"
    );
    let fresh = FastBuildLock::check_freshness_with_options(
        &mitts::core::FreshnessCheckArgs {
            project_dir: &r3_dir,
            profile: "dev",
            package: &None,
            features: &[],
            no_default_features: false,
            all_features: false,
            toolchain_version: &lock.toolchain_version,
        },
        Some(&r3_dir.join("target")),
        lock.options_hash.as_deref(),
        true,
    );
    assert!(
        matches!(fresh, mitts::core::FreshnessResult::Stale(ref msg) if msg.contains("Incomplete tracked inputs")),
        "Expected Stale(Incomplete tracked inputs), got {:?}",
        fresh
    );
}

#[test]
fn test_n1_n3_dep_info_escaped_spaces_and_safe_manifest_atomic_save() {
    let dir = tempdir().unwrap();
    let dep_file = dir.path().join("unit.d");
    fs::write(
        &dep_file,
        "/tmp/out/my\\ bin: /tmp/ws/src/main.rs /tmp/ws/assets/my\\ file\\ name.json \\\n  /tmp/ws/other\\ dir/mod.rs\n# env-dep:MY_ENV=1\n",
    )
    .unwrap();
    let parsed = parse_dep_info_full(&dep_file, None).unwrap();
    assert_eq!(
        parsed.files,
        vec![
            PathBuf::from("/tmp/ws/src/main.rs"),
            PathBuf::from("/tmp/ws/assets/my file name.json"),
            PathBuf::from("/tmp/ws/other dir/mod.rs"),
        ]
    );
    assert_eq!(
        parsed.env_deps,
        vec![("MY_ENV".to_string(), Some("1".to_string()))]
    );

    let dest_manifest = dir.path().join("manifest.json");
    let manifest = ActionManifest {
        base_key: "key/with:special..chars".to_string(),
        entries: vec![ActionManifestEntry {
            extra_inputs: Vec::new(),
            result_key: "result_hash".to_string(),
        }],
    };
    manifest.save_atomic(&dest_manifest, dir.path()).unwrap();
    let loaded = ActionManifest::from_file(&dest_manifest).unwrap().unwrap();
    assert_eq!(loaded.entries[0].result_key, "result_hash");
}

#[test]
fn test_f1_f3_host_debuginfo_default_and_rpath_false() {
    let _guard = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempdir().unwrap();
    unsafe {
        std::env::set_var("CARGO_HOME", dir.path().join(".cargo-home"));
        std::env::remove_var("CARGO_PROFILE_DEV_DEBUG");
        std::env::remove_var("CARGO_PROFILE_RELEASE_DEBUG");
    }

    // 1. Host unit in dev defaults to debuginfo = 0 and does NOT inherit top-level [profile.dev] debug = 2
    let manifest = dir.path().join("Cargo.toml");
    fs::write(
        &manifest,
        r#"[package]
name = "f1_pkg"
version = "0.1.0"
edition = "2021"

[profile.dev]
debug = 2

[profile.release]
rpath = false
"#,
    )
    .unwrap();
    let target_prof =
        resolve_unit_profile(&manifest, "f1_pkg", true, false, false, Some(dir.path())).unwrap();
    assert_eq!(target_prof.debuginfo, 2);
    let host_prof =
        resolve_unit_profile(&manifest, "f1_pkg", true, true, false, Some(dir.path())).unwrap();
    assert_eq!(
        host_prof.debuginfo, 0,
        "Host unit must default to debuginfo = 0 and not inherit top-level [profile.dev] debug"
    );

    // 2. [profile.release] rpath = false is accepted as a no-op
    let rel_prof =
        resolve_unit_profile(&manifest, "f1_pkg", true, false, true, Some(dir.path())).unwrap();
    assert_eq!(rel_prof.opt_level, "3");

    // 3. [profile.release] rpath = true is rejected as UnsupportedOption
    fs::write(
        &manifest,
        r#"[package]
name = "f1_pkg"
version = "0.1.0"
edition = "2021"

[profile.release]
rpath = true
"#,
    )
    .unwrap();
    let err =
        resolve_unit_profile(&manifest, "f1_pkg", true, false, true, Some(dir.path())).unwrap_err();
    let adapter_err = err
        .downcast_ref::<RustAdapterError>()
        .expect("Expected RustAdapterError");
    assert!(
        matches!(adapter_err, RustAdapterError::UnsupportedOption(msg) if msg.contains("profile.release.rpath")),
        "Expected UnsupportedOption for rpath = true, got {:?}",
        adapter_err
    );
}
