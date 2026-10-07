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
    extract_dependencies_with_reqs, extract_root_package, NativeResolver,
};
use mitts::adapter::rust::sparse_index::SparseIndexClient;
use std::fs;
use tempfile::TempDir;

#[test]
fn test_m8_sparse_index_client_paths_and_offline_cache() {
    let tmp = TempDir::new().unwrap();
    let cache_dir = tmp.path().join("index");
    let client = SparseIndexClient::with_cache_dir(cache_dir.clone()).unwrap();

    // 1. Relative path edge cases
    assert_eq!(SparseIndexClient::relative_path(""), "");
    assert_eq!(SparseIndexClient::relative_path("x"), "1/x");
    assert_eq!(SparseIndexClient::relative_path("yo"), "2/yo");
    assert_eq!(SparseIndexClient::relative_path("bar"), "3/b/bar");
    assert_eq!(SparseIndexClient::relative_path("serde"), "se/rd/serde");

    // 2. Empty crate name error
    assert!(client.fetch_crate_versions("", true).is_err());

    // 3. Offline cache miss
    let offline_err = client.fetch_crate_versions("missing-crate", true);
    assert!(offline_err.is_err());
    assert!(offline_err.unwrap_err().to_string().contains("--offline requested"));

    // 4. Offline cache hit
    let rel_path = SparseIndexClient::relative_path("cached-crate");
    let full_path = cache_dir.join(&rel_path);
    fs::create_dir_all(full_path.parent().unwrap()).unwrap();
    let sample_entry = r#"{"name":"cached-crate","vers":"1.0.0","deps":[],"cksum":"deadbeef","yanked":false}"#;
    fs::write(&full_path, format!("{}\n", sample_entry)).unwrap();

    let entries = client.fetch_crate_versions("cached-crate", true).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "cached-crate");
    assert_eq!(entries[0].vers, "1.0.0");
}

#[test]
fn test_m8_resolver_extract_helpers_and_path_deps() {
    let manifest_str = r#"
[package]
name = "resolver-root"
version = "0.4.2"
edition = "2021"

[dependencies]
local_sub = { path = "sub", version = "0.2.0" }
simple_dep = "1.0"
detailed_dep = { version = "2.1", features = ["foo"] }

[build-dependencies]
build_tool = { version = "0.5" }

[target.'cfg(unix)'.dependencies]
unix_lib = "0.3"
"#;

    let (root_name, root_ver) = extract_root_package(manifest_str).unwrap();
    assert_eq!(root_name, "resolver-root");
    assert_eq!(root_ver, "0.4.2");

    let deps = extract_dependencies_with_reqs(manifest_str);
    assert!(deps.iter().any(|d| d.name == "simple_dep" && d.req == "1.0"));
    assert!(deps.iter().any(|d| d.name == "detailed_dep" && d.req == "2.1"));
    assert!(deps.iter().any(|d| d.name == "build_tool" && d.req == "0.5"));
    assert!(deps.iter().any(|d| d.name == "unix_lib" && d.req == "0.3"));
}

#[test]
fn test_m8_resolver_resolve_manifest_with_path_deps() {
    let tmp = TempDir::new().unwrap();
    let root_dir = tmp.path();

    // Create sub path dependency
    let sub_dir = root_dir.join("sub");
    fs::create_dir_all(&sub_dir).unwrap();
    fs::write(
        sub_dir.join("Cargo.toml"),
        "[package]\nname = \"sub-crate\"\nversion = \"0.2.0\"\n",
    )
    .unwrap();

    // Create root manifest
    let root_toml = root_dir.join("Cargo.toml");
    fs::write(
        &root_toml,
        r#"
[package]
name = "root-crate"
version = "0.1.0"

[dependencies]
sub-crate = { path = "sub" }
"#,
    )
    .unwrap();

    let client = SparseIndexClient::with_cache_dir(tmp.path().join("index")).unwrap();
    let resolver = NativeResolver::with_client(client);

    let lock = resolver.resolve_manifest(&root_toml, true).unwrap();
    assert_eq!(lock.version, 3);
    assert!(lock.packages.iter().any(|p| p.name == "sub-crate" && p.version == "0.2.0"));
    assert!(lock.packages.iter().any(|p| p.name == "root-crate" && p.version == "0.1.0"));
}
