/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::rust::lockfile::{CargoLock, CargoLockPackage};
use mitts::adapter::rust::manifest::CargoToml;
use mitts::adapter::rust::package_selection::select_transitive_packages;
use mitts::adapter::rust::types::RustBuildOptions;
use mitts::core::action_cache::ActionCache;
use mitts::core::cas::ContentAddressedStore;
use mitts::core::gc::{CacheGc, GcOptions};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

#[test]
fn test_m8_package_selection_target_not_found() {
    let lockfile = CargoLock {
        version: 3,
        packages: vec![CargoLockPackage {
            name: "crate-a".to_string(),
            version: "0.1.0".to_string(),
            source: None,
            checksum: None,
            dependencies: vec![],
        }],
    };
    let manifests = HashMap::new();
    let root_toml = CargoToml::default();
    let opt = RustBuildOptions {
        package: Some("non-existent-crate".to_string()),
        ..RustBuildOptions::default()
    };

    let res = select_transitive_packages(
        &lockfile,
        &manifests,
        &root_toml,
        &PathBuf::from("."),
        &opt,
    );
    assert!(res.is_err(), "Non-existent package must return error");
    let err_str = res.unwrap_err().to_string();
    assert!(err_str.contains("Package 'non-existent-crate' not found"));
}

#[test]
fn test_m8_package_selection_transitive_graph() {
    let pkg_c = CargoLockPackage {
        name: "dep-c".to_string(),
        version: "0.3.0".to_string(),
        source: None,
        checksum: None,
        dependencies: vec![],
    };
    let pkg_b = CargoLockPackage {
        name: "dep-b".to_string(),
        version: "0.2.0".to_string(),
        source: None,
        checksum: None,
        dependencies: vec!["dep-c 0.3.0".to_string()],
    };
    let pkg_a = CargoLockPackage {
        name: "crate-a".to_string(),
        version: "0.1.0".to_string(),
        source: None,
        checksum: None,
        dependencies: vec!["dep-b 0.2.0".to_string()],
    };

    let lockfile = CargoLock {
        version: 3,
        packages: vec![pkg_a.clone(), pkg_b.clone(), pkg_c.clone()],
    };

    let mut manifests = HashMap::new();
    manifests.insert("crate-a".to_string(), (PathBuf::from("crates/a"), CargoToml::default()));

    let root_toml = CargoToml::default();
    let opt = RustBuildOptions {
        package: Some("crate-a".to_string()),
        ..RustBuildOptions::default()
    };

    let selected = select_transitive_packages(
        &lockfile,
        &manifests,
        &root_toml,
        &PathBuf::from("."),
        &opt,
    ).expect("select transitive");

    let names: Vec<&str> = selected.iter().map(|(p, _)| p.name.as_str()).collect();
    assert!(names.contains(&"crate-a"));
    assert!(names.contains(&"dep-b"));
    assert!(names.contains(&"dep-c"));

    // crate-a is in workspace_manifests, dep-b and dep-c are external
    let ws_flags: HashMap<&str, bool> = selected
        .iter()
        .map(|(p, is_ws)| (p.name.as_str(), *is_ws))
        .collect();
    assert_eq!(ws_flags.get("crate-a"), Some(&true));
    assert_eq!(ws_flags.get("dep-b"), Some(&false));
    assert_eq!(ws_flags.get("dep-c"), Some(&false));
}

#[test]
fn test_m8_cache_gc_pruning() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let cas_dir = temp_dir.path().join("cas");
    let ac_dir = temp_dir.path().join("ac");

    let cas = ContentAddressedStore::new(cas_dir).unwrap();
    let ac = ActionCache::new(ac_dir).unwrap();

    // Store objects into CAS
    let data1 = b"object 1 content";
    let data2 = b"object 2 content that is a bit longer";
    let (_h1, _) = cas.store_bytes(data1).unwrap();
    let (_h2, _) = cas.store_bytes(data2).unwrap();

    let gc = CacheGc::new(&cas, &ac);

    // Prune with very low max_bytes (e.g. 5 bytes) to force pruning
    let opts = GcOptions {
        max_bytes: Some(5),
        max_age: None,
    };
    let report = gc.prune(&opts).expect("run gc");
    assert_eq!(report.objects_examined, 2);
    assert!(report.objects_pruned >= 1);
    assert!(report.bytes_freed > 0);

    // Prune with max_age = 1 day (should retain remaining object)
    let opts_age = GcOptions {
        max_bytes: None,
        max_age: Some(Duration::from_secs(86400)),
    };
    let report2 = gc.prune(&opts_age).expect("run gc age");
    assert_eq!(report2.objects_pruned, 0);
}
