/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! The planner looks a package up in Cargo's registry once per package; the registry's
//! index directories are listed once per process, not once per lookup.

use mitts::core::source_store::RegistryLookup;
use std::fs;
use tempfile::TempDir;

#[test]
fn registry_index_dirs_are_listed_once_per_process() {
    let home = TempDir::new().unwrap();
    let src = home.path().join("registry/src");
    fs::create_dir_all(src.join("index.crates.io-1111111111111111")).unwrap();
    fs::create_dir_all(src.join("not-a-registry")).unwrap();
    let exclude = home.path().join("elsewhere");

    let first = RegistryLookup::discover_with_home(Some(home.path().to_path_buf()), &exclude);
    assert_eq!(first.src_index_dirs().len(), 1);

    fs::create_dir_all(src.join("index.crates.io-2222222222222222")).unwrap();
    let again = RegistryLookup::discover_with_home(Some(home.path().to_path_buf()), &exclude);
    assert_eq!(
        again.src_index_dirs(),
        first.src_index_dirs(),
        "a process keeps its first listing"
    );
    assert_eq!(
        RegistryLookup::scan(Some(home.path()), &exclude)
            .src_index_dirs()
            .len(),
        2,
        "a fresh scan sees the new index"
    );

    // The exclusion keeps Mitts' own store out of the registry listing.
    let own_store = src.join("index.crates.io-1111111111111111");
    let excluded = RegistryLookup::scan(Some(home.path()), &own_store);
    assert!(!excluded.src_index_dirs().contains(&own_store));
}
