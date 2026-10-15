/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::rust::fetcher::{CrateFetcher, FetchOptions};
use mitts::adapter::rust::inputs::hash_package_base_inputs;
use mitts::adapter::rust::lockfile::CargoLockPackage;
use mitts::adapter::rust::sparse_index::{SparseIndexClient, SparseIndexDep};
use mitts::core::source_store::SourceStore;
use std::fs;
use std::sync::Mutex;
use tempfile::TempDir;

static ENV_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn test_m8_sparse_index_dep_default_and_empty_cache() {
    // 1. SparseIndexDep deserialization without default_features exercises default_true()
    let json_dep = r#"{"name": "serde", "req": "1.0"}"#;
    let dep: SparseIndexDep = serde_json::from_str(json_dep).unwrap();
    assert_eq!(dep.name, "serde");
    assert!(dep.default_features);

    // 2. Cached file with empty content / whitespace
    let temp = TempDir::new().unwrap();
    let client = SparseIndexClient::with_cache_dir(temp.path().to_path_buf()).unwrap();

    let rel_path = SparseIndexClient::relative_path("empty_crate_pkg");
    let cache_file = temp.path().join(rel_path);
    fs::create_dir_all(cache_file.parent().unwrap()).unwrap();
    fs::write(&cache_file, " \n\n ").unwrap();

    // In offline mode with empty cached file, it fails as expected
    let res = client.fetch_crate_versions("empty_crate_pkg", true);
    assert!(res.is_err());

    // 3. In online mode, 404 response for nonexistent crate
    let res_online =
        client.fetch_crate_versions("crate_that_absolutely_does_not_exist_xyz_9999", false);
    assert!(res_online.is_err());
}

#[test]
fn test_m8_fetcher_no_cargo_cache_and_recheck_lock() {
    let temp = TempDir::new().unwrap();
    let store_dir = temp.path().join("sources");
    let store = SourceStore::new(store_dir.clone()).unwrap();
    let fetcher = CrateFetcher::new(store.clone());

    let pkg = CargoLockPackage {
        name: "test_pkg_nonexistent_xyz_12345".to_string(),
        version: "1.0.0".to_string(),
        source: Some("registry+https://github.com/rust-lang/crates.io-index".to_string()),
        checksum: Some(
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".to_string(),
        ),
        dependencies: Vec::new(),
    };

    // 1. Offline fetch fails because not in cache or src
    let opt_offline = FetchOptions {
        offline: true,
        ..Default::default()
    };
    let res_offline = fetcher.fetch_package(&pkg, &opt_offline);
    assert!(res_offline.is_err());

    // 2. Populate source store directly, then fetch again (exercises re-check inside lock)
    let pkg_dir = store.package_dir("test_pkg_nonexistent_xyz_12345", "1.0.0");
    fs::create_dir_all(&pkg_dir).unwrap();
    fs::write(
        pkg_dir.join("Cargo.toml"),
        "[package]\nname = \"test_pkg_nonexistent_xyz_12345\"\nversion = \"1.0.0\"\n",
    )
    .unwrap();
    fs::write(pkg_dir.join(".mitts-ok"), "ok\n").unwrap();

    let res_cached = fetcher.fetch_package(&pkg, &opt_offline);
    assert!(res_cached.is_ok());
    assert_eq!(res_cached.unwrap(), Some(pkg_dir));
}

#[test]
fn test_m8_inputs_home_configs_tmp_files_and_mitts_sources() {
    let _lock = ENV_LOCK.lock().unwrap();
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    // 1. Create a package with temporary swap and tmp files
    let pkg_dir = root.join("my_pkg");
    fs::create_dir_all(pkg_dir.join("src")).unwrap();
    fs::write(
        pkg_dir.join("Cargo.toml"),
        "[package]\nname = \"my_pkg\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(pkg_dir.join("src/lib.rs"), "pub fn run() {}\n").unwrap();
    fs::write(pkg_dir.join("src/temp.tmp"), "temp file").unwrap();
    fs::write(pkg_dir.join("src/swap.swp"), "swap file").unwrap();
    fs::write(pkg_dir.join("src/Cargo.lock"), "lock inside src").unwrap();

    // 2. Configure CARGO_HOME with config without touching HOME
    let fake_cargo_home = root.join("fake_cargo_home");
    fs::create_dir_all(&fake_cargo_home).unwrap();
    fs::write(fake_cargo_home.join("config"), "[target]\n").unwrap();

    let old_cargo_home = std::env::var("CARGO_HOME").ok();
    std::env::set_var("CARGO_HOME", &fake_cargo_home);

    let inputs = hash_package_base_inputs(&pkg_dir, None, true).unwrap();
    assert!(inputs.iter().any(|(k, _)| k.contains("CARGO_HOME/config")));
    assert!(inputs.iter().any(|(k, _)| k == "src/lib.rs"));
    assert!(!inputs.iter().any(|(k, _)| k.ends_with(".tmp")));
    assert!(!inputs.iter().any(|(k, _)| k.ends_with(".swp")));

    // When config is replaced by config.toml, config.toml is discovered
    fs::remove_file(fake_cargo_home.join("config")).unwrap();
    fs::write(fake_cargo_home.join("config.toml"), "[build]\njobs = 4\n").unwrap();
    let inputs_toml = hash_package_base_inputs(&pkg_dir, None, true).unwrap();
    assert!(inputs_toml
        .iter()
        .any(|(k, _)| k.contains("CARGO_HOME/config.toml")));

    if let Some(ch) = old_cargo_home {
        std::env::set_var("CARGO_HOME", ch);
    } else {
        std::env::remove_var("CARGO_HOME");
    }

    // 3. Fast-path in MITTS_CACHE_DIR/sources
    let mitts_cache = root.join("mitts_cache");
    let source_pkg = mitts_cache.join("sources").join("remote_dep-1.0.0");
    fs::create_dir_all(&source_pkg).unwrap();
    fs::write(
        source_pkg.join("Cargo.toml"),
        "[package]\nname = \"remote_dep\"\nversion = \"1.0.0\"\nedition = \"2021\"\n",
    )
    .unwrap();

    std::env::set_var("MITTS_CACHE_DIR", &mitts_cache);
    let fast_inputs = hash_package_base_inputs(&source_pkg, None, false).unwrap();
    assert!(fast_inputs.iter().any(|(k, _)| k == "immutable_pkg"));
    std::env::remove_var("MITTS_CACHE_DIR");
}
