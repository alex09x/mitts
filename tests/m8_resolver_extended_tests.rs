/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::rust::resolver::{
    extract_dependencies_with_reqs, extract_root_package, ManifestDirectDep, NativeResolver,
};
use mitts::adapter::rust::sparse_index::{SparseIndexClient, SparseIndexDep, SparseIndexEntry};
use std::fs;
use tempfile::TempDir;

#[test]
fn test_m8_extract_root_package_and_dependencies_advanced() {
    // 1. Missing name or missing version returns None
    assert!(extract_root_package("[package]\nname = \"foo\"\n").is_none());
    assert!(extract_root_package("[package]\nversion = \"0.1.0\"\n").is_none());
    assert!(extract_root_package("# Just a comment\n").is_none());

    // 2. Valid root package with comments
    let manifest = r#"
# Comment
[package]
name = "my-root-pkg" # inline comment
version = "0.2.5"

[dependencies]
dep_a = "1.0"
dep_b = { version = "2.3.4", features = ["std"] }

[build-dependencies]
build_dep = { version = "0.4" }

[target.'cfg(unix)'.dependencies]
unix_dep = "0.5"
"#;
    let (name, ver) = extract_root_package(manifest).unwrap();
    assert_eq!(name, "my-root-pkg");
    assert_eq!(ver, "0.2.5");

    let deps = extract_dependencies_with_reqs(manifest);
    assert_eq!(deps.len(), 4);
    assert!(deps.iter().any(|d| d.name == "dep_a" && d.req == "1.0"));
    assert!(deps.iter().any(|d| d.name == "dep_b" && d.req == "2.3.4"));
    assert!(deps.iter().any(|d| d.name == "build_dep" && d.req == "0.4"));
    assert!(deps.iter().any(|d| d.name == "unix_dep" && d.req == "0.5"));
}

#[test]
fn test_m8_resolver_yanked_and_sub_dependencies() {
    let tmp = TempDir::new().unwrap();
    let cache_dir = tmp.path().join("index_cache");
    fs::create_dir_all(&cache_dir).unwrap();

    // Populate fake sparse index files in cache
    // 1. crate "dep_with_sub": has versions 1.0.0 (yanked), 1.1.0 (valid, depends on "sub_dep"), 1.2.0 (yanked)
    let p_sub = cache_dir.join("de").join("p_").join("dep_with_sub");
    fs::create_dir_all(p_sub.parent().unwrap()).unwrap();

    let e_1_0 = serde_json::to_string(&SparseIndexEntry {
        name: "dep_with_sub".to_string(),
        vers: "1.0.0".to_string(),
        deps: Vec::new(),
        cksum: "cksum100".to_string(),
        yanked: true,
        features: Default::default(),
    })
    .unwrap();

    let e_1_1 = serde_json::to_string(&SparseIndexEntry {
        name: "dep_with_sub".to_string(),
        vers: "1.1.0".to_string(),
        deps: vec![
            SparseIndexDep {
                name: "sub_dep".to_string(),
                req: "0.1".to_string(),
                features: Vec::new(),
                optional: false,
                default_features: true,
                target: None,
                kind: Some("normal".to_string()),
                package: None,
            },
            SparseIndexDep {
                name: "dev_dep".to_string(),
                req: "0.1".to_string(),
                features: Vec::new(),
                optional: false,
                default_features: true,
                target: None,
                kind: Some("dev".to_string()),
                package: None,
            },
            SparseIndexDep {
                name: "opt_dep".to_string(),
                req: "0.1".to_string(),
                features: Vec::new(),
                optional: true,
                default_features: true,
                target: None,
                kind: None,
                package: None,
            },
        ],
        cksum: "cksum110".to_string(),
        yanked: false,
        features: Default::default(),
    })
    .unwrap();

    let e_1_2 = serde_json::to_string(&SparseIndexEntry {
        name: "dep_with_sub".to_string(),
        vers: "1.2.0".to_string(),
        deps: Vec::new(),
        cksum: "cksum120".to_string(),
        yanked: true,
        features: Default::default(),
    })
    .unwrap();

    fs::write(&p_sub, format!("{}\n{}\n{}\n", e_1_0, e_1_1, e_1_2)).unwrap();

    // 2. crate "sub_dep": version 0.1.5
    let p_subdep = cache_dir.join("su").join("b_").join("sub_dep");
    fs::create_dir_all(p_subdep.parent().unwrap()).unwrap();
    let e_sub = serde_json::to_string(&SparseIndexEntry {
        name: "sub_dep".to_string(),
        vers: "0.1.5".to_string(),
        deps: Vec::new(),
        cksum: "cksum_sub".to_string(),
        yanked: false,
        features: Default::default(),
    })
    .unwrap();
    fs::write(&p_subdep, format!("{}\n", e_sub)).unwrap();

    let client = SparseIndexClient::with_cache_dir(cache_dir).unwrap();
    let resolver = NativeResolver::with_client(client);

    let root_deps = vec![ManifestDirectDep {
        name: "dep_with_sub".to_string(),
        req: "^1.0".to_string(),
    }];

    let lock = resolver.resolve_dependencies(&root_deps, true).unwrap();
    assert_eq!(lock.packages.len(), 2);

    let pkg_main = lock
        .packages
        .iter()
        .find(|p| p.name == "dep_with_sub")
        .unwrap();
    assert_eq!(
        pkg_main.version, "1.1.0",
        "Must pick non-yanked 1.1.0 instead of yanked 1.2.0"
    );
    assert_eq!(
        pkg_main.dependencies,
        vec!["sub_dep"],
        "Must exclude dev and optional deps"
    );

    let pkg_sub = lock.packages.iter().find(|p| p.name == "sub_dep").unwrap();
    assert_eq!(pkg_sub.version, "0.1.5");
}
