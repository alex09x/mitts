/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::rust::inputs::{append_unit_dep_info_inputs, hash_package_base_inputs};
use mitts::adapter::rust::types::RustBuildOptions;
use mitts::adapter::RustAdapter;
use mitts::core::build_lock::{
    FastBuildLock, FreshnessCheckArgs, FreshnessResult, RecordStateArgs,
};
use std::fs;
use std::sync::Mutex;
use tempfile::TempDir;

static SERIAL_TEST_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn test_m8_inputs_cargo_home_and_ancestors() {
    let _lock = SERIAL_TEST_LOCK.lock().unwrap();
    let temp_dir = TempDir::new().expect("tempdir");
    let ws_dir = temp_dir.path().join("workspace");
    let crate_dir = ws_dir.join("crates").join("my_crate");
    let src_dir = crate_dir.join("src");
    fs::create_dir_all(&src_dir).unwrap();

    let ws_manifest = ws_dir.join("Cargo.toml");
    fs::write(&ws_manifest, "[workspace]\nmembers = [\"crates/*\"]\n").unwrap();

    let ws_cargo_dir = ws_dir.join(".cargo");
    fs::create_dir_all(&ws_cargo_dir).unwrap();
    fs::write(ws_cargo_dir.join("config"), "# bare config\n").unwrap();

    let crate_manifest = crate_dir.join("Cargo.toml");
    fs::write(
        &crate_manifest,
        "[package]\nname = \"my_crate\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(src_dir.join("lib.rs"), "pub fn run() {}\n").unwrap();

    // 1. CARGO_HOME with config.toml and config
    let fake_cargo_home = temp_dir.path().join("cargo_home");
    fs::create_dir_all(&fake_cargo_home).unwrap();
    fs::write(fake_cargo_home.join("config.toml"), "# home toml\n").unwrap();
    fs::write(fake_cargo_home.join("config"), "# home bare\n").unwrap();

    let old_ch = std::env::var("CARGO_HOME").ok();
    std::env::set_var("CARGO_HOME", &fake_cargo_home);

    let inputs = hash_package_base_inputs(&crate_dir, Some(&ws_manifest), true).unwrap();
    let keys: Vec<String> = inputs.into_iter().map(|(k, _)| k).collect();

    assert!(keys.iter().any(|k| k == "CARGO_HOME/config.toml"));
    assert!(keys.iter().any(|k| k == "CARGO_HOME/config"));
    assert!(keys.iter().any(|k| k == "workspace:Cargo.toml"));
    assert!(keys.iter().any(|k| k == "workspace:.cargo/config"));

    if let Some(ch) = old_ch {
        std::env::set_var("CARGO_HOME", ch);
    } else {
        std::env::remove_var("CARGO_HOME");
    }

    // 2. MITTS_CACHE_DIR immutable registry detection
    let mitts_cache = temp_dir.path().join("mitts_cache");
    let sources_crate = mitts_cache.join("sources").join("remote-crate-1.0.0");
    fs::create_dir_all(&sources_crate).unwrap();
    fs::write(
        sources_crate.join("Cargo.toml"),
        "[package]\nname = \"remote-crate\"\nversion = \"1.0.0\"\n",
    )
    .unwrap();
    fs::write(sources_crate.join(".mitts-ok"), "").unwrap();

    let old_mc = std::env::var("MITTS_CACHE_DIR").ok();
    std::env::set_var("MITTS_CACHE_DIR", &mitts_cache);

    let imm_inputs = hash_package_base_inputs(&sources_crate, None, false).unwrap();
    assert!(imm_inputs.iter().any(|(k, _)| k == "immutable_pkg"));

    if let Some(mc) = old_mc {
        std::env::set_var("MITTS_CACHE_DIR", mc);
    } else {
        std::env::remove_var("MITTS_CACHE_DIR");
    }
}

#[test]
fn test_m8_build_lock_sha_mtime_tolerance() {
    let _lock = SERIAL_TEST_LOCK.lock().unwrap();
    let temp_dir = TempDir::new().expect("tempdir");
    let proj_dir = temp_dir.path();
    let manifest = proj_dir.join("Cargo.toml");
    let lock_file = proj_dir.join("Cargo.lock");
    let src_dir = proj_dir.join("src");
    fs::create_dir_all(&src_dir).unwrap();

    fs::write(
        &manifest,
        "[package]\nname = \"lock-sha\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(&lock_file, "version = 3\n").unwrap();
    fs::write(src_dir.join("main.rs"), "fn main() {}\n").unwrap();

    let out_file = proj_dir.join("target").join("bin");
    fs::create_dir_all(out_file.parent().unwrap()).unwrap();
    fs::write(&out_file, b"binary").unwrap();

    let outputs = vec![out_file.clone()];
    let rec_args = RecordStateArgs {
        project_dir: proj_dir,
        profile: "dev",
        package: &None,
        features: &[],
        no_default_features: false,
        all_features: false,
        toolchain_version: "rustc 1.80.0",
        action_count: 1,
        root_binary: Some(out_file.clone()),
        outputs: &outputs,
        stderr: None,
    };

    FastBuildLock::record_state(&rec_args).unwrap();

    let chk_args = FreshnessCheckArgs {
        project_dir: proj_dir,
        profile: "dev",
        package: &None,
        features: &[],
        no_default_features: false,
        all_features: false,
        toolchain_version: "rustc 1.80.0",
    };

    // 1. Initially fresh
    assert!(matches!(
        FastBuildLock::check_freshness(&chk_args),
        FreshnessResult::Fresh { .. }
    ));

    // 2. Touch Cargo.lock (same content, changed mtime) -> sha matches, remains Fresh
    std::thread::sleep(std::time::Duration::from_millis(15));
    fs::write(&lock_file, "version = 3\n").unwrap();
    assert!(matches!(
        FastBuildLock::check_freshness(&chk_args),
        FreshnessResult::Fresh { .. }
    ));

    // 3. Modify Cargo.lock content -> Stale
    fs::write(&lock_file, "version = 3\n# modified\n").unwrap();
    assert!(matches!(
        FastBuildLock::check_freshness(&chk_args),
        FreshnessResult::Stale(_)
    ));

    // 4. Reset Cargo.lock, touch Cargo.toml (same content, changed mtime) -> Fresh
    fs::write(&lock_file, "version = 3\n").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(15));
    fs::write(
        &manifest,
        "[package]\nname = \"lock-sha\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    assert!(matches!(
        FastBuildLock::check_freshness(&chk_args),
        FreshnessResult::Fresh { .. }
    ));

    // 5. Modify Cargo.toml content -> Stale
    fs::write(
        &manifest,
        "[package]\nname = \"lock-sha\"\nversion = \"0.2.0\"\n",
    )
    .unwrap();
    assert!(matches!(
        FastBuildLock::check_freshness(&chk_args),
        FreshnessResult::Stale(_)
    ));
}

#[test]
fn test_m8_planner_explain_and_dep_info() {
    let isolated_cache = tempfile::tempdir().expect("isolated cache dir");
    let temp_dir = TempDir::new().expect("tempdir");
    let manifest = temp_dir.path().join("Cargo.toml");
    let src_dir = temp_dir.path().join("src");
    fs::create_dir_all(&src_dir).unwrap();

    fs::write(
        &manifest,
        r#"[package]
name = "multilib"
version = "0.1.0"

[lib]
crate-type = ["cdylib", "staticlib"]
"#,
    )
    .unwrap();
    fs::write(src_dir.join("lib.rs"), "pub extern \"C\" fn entry() {}\n").unwrap();

    let opt = RustBuildOptions {
        cache_dir: Some(isolated_cache.path().to_path_buf()),
        manifest_path: Some(manifest.clone()),
        target_dir: Some(temp_dir.path().join("target")),
        ..Default::default()
    };
    let explanation = RustAdapter::explain_plan(&opt).unwrap();
    assert!(!explanation.units.is_empty());

    // Test append_unit_dep_info_inputs with out-of-package asset
    let target_debug = temp_dir.path().join("target").join("debug");
    fs::create_dir_all(&target_debug).unwrap();

    let external_asset = temp_dir.path().join("external_data.txt");
    fs::write(&external_asset, "external payload").unwrap();

    let dep_info = target_debug.join("libmultilib.d");
    fs::write(
        &dep_info,
        format!(
            "{}: {} {}\n",
            target_debug.join("libmultilib.so").display(),
            src_dir.join("lib.rs").display(),
            external_asset.display()
        ),
    )
    .unwrap();

    let base = hash_package_base_inputs(temp_dir.path(), None, false).unwrap();
    let updated =
        append_unit_dep_info_inputs(base, temp_dir.path(), &target_debug, "multilib").unwrap();
    assert!(updated.iter().any(|(k, _)| k.contains("external_data.txt")));
}
