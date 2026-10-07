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
use mitts::adapter::rust::manifest::{CargoToml, TomlWorkspace};
use mitts::adapter::rust::package_selection::{
    is_target_incompatible_with_host, parse_dep_spec, select_transitive_packages,
};
use mitts::adapter::rust::source_lookup::find_crate_source_dir;
use mitts::adapter::rust::types::RustBuildOptions;
use std::collections::HashMap;
use std::fs;
use tempfile::TempDir;

#[test]
fn test_m8_package_selection_default_members_and_incompatible() {
    let tmp = TempDir::new().unwrap();
    let p = tmp.path();

    let member_a_dir = p.join("member_a");
    fs::create_dir_all(&member_a_dir).unwrap();
    let member_a_toml = CargoToml::default();

    let mut manifests = HashMap::new();
    manifests.insert("member_a".to_string(), (member_a_dir, member_a_toml));

    let root_toml = CargoToml {
        workspace: Some(TomlWorkspace {
            members: vec!["member_a".to_string()],
            default_members: vec!["member_a".to_string()],
            exclude: Vec::new(),
            ..Default::default()
        }),
        ..Default::default()
    };

    let lockfile = CargoLock {
        version: 3,
        packages: vec![
            CargoLockPackage {
                name: "member_a".to_string(),
                version: "0.1.0".to_string(),
                source: None,
                checksum: None,
                dependencies: vec![
                    "dep_normal 0.2.0".to_string(),
                    "windows-sys 0.48.0".to_string(),
                    "web-sys 0.3.60".to_string(),
                ],
            },
            CargoLockPackage {
                name: "dep_normal".to_string(),
                version: "0.2.0".to_string(),
                source: None,
                checksum: None,
                dependencies: Vec::new(),
            },
            CargoLockPackage {
                name: "windows-sys".to_string(),
                version: "0.48.0".to_string(),
                source: None,
                checksum: None,
                dependencies: Vec::new(),
            },
            CargoLockPackage {
                name: "web-sys".to_string(),
                version: "0.3.60".to_string(),
                source: None,
                checksum: None,
                dependencies: Vec::new(),
            },
        ],
    };

    let opt = RustBuildOptions::default();
    let selected = select_transitive_packages(&lockfile, &manifests, &root_toml, p, &opt).unwrap();
    assert!(selected.iter().any(|(pkg, is_ws)| pkg.name == "member_a" && *is_ws));
    assert!(selected.iter().any(|(pkg, is_ws)| pkg.name == "dep_normal" && !*is_ws));

    // Test helper parse_dep_spec
    let (n1, v1) = parse_dep_spec("serde 1.0.190");
    assert_eq!(n1, "serde");
    assert_eq!(v1, Some("1.0.190"));
    let (n2, v2) = parse_dep_spec("simple_dep");
    assert_eq!(n2, "simple_dep");
    assert_eq!(v2, None);

    // Test platform incompatibilities
    assert!(is_target_incompatible_with_host("orbclient"));
    assert!(is_target_incompatible_with_host("web-sys"));
    assert!(is_target_incompatible_with_host("js-sys"));
    assert!(is_target_incompatible_with_host("wasm-bindgen-futures"));
    assert!(is_target_incompatible_with_host("ndk"));
}

#[test]
fn test_m8_source_lookup_git_crates_and_fallbacks() {
    let tmp = TempDir::new().unwrap();
    let fake_home = tmp.path().join("home");
    fs::create_dir_all(&fake_home).unwrap();

    let old_home = std::env::var("HOME").ok();
    std::env::set_var("HOME", &fake_home);

    // 1. Setup git checkouts structure with nested crates/
    let git_dir = fake_home
        .join(".cargo")
        .join("git")
        .join("checkouts")
        .join("my-repo-abc1234")
        .join("abcdef123456");
    let nested_crate = git_dir.join("crates").join("nested-sub");
    fs::create_dir_all(&nested_crate).unwrap();
    fs::write(
        nested_crate.join("Cargo.toml"),
        "[package]\nname = \"nested-sub\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();

    // Find git crate with matching commit hash in source url
    let found = find_crate_source_dir(
        "nested-sub",
        "0.1.0",
        Some("git+https://github.com/org/my-repo#abcdef1234567890"),
    );
    assert_eq!(found, Some(nested_crate));

    // Mismatched commit hash returns None
    let not_found = find_crate_source_dir(
        "nested-sub",
        "0.1.0",
        Some("git+https://github.com/org/my-repo#deadbeef0000"),
    );
    assert_eq!(not_found, None);

    // 2. Setup standard mitts cache location
    let mitts_src = fake_home
        .join(".cache")
        .join("mitts")
        .join("sources")
        .join("cached-dep-0.5.0");
    fs::create_dir_all(&mitts_src).unwrap();
    fs::write(
        mitts_src.join("Cargo.toml"),
        "[package]\nname = \"cached-dep\"\nversion = \"0.5.0\"\n",
    )
    .unwrap();

    let found_mitts = find_crate_source_dir("cached-dep", "0.5.0", None);
    assert_eq!(found_mitts, Some(mitts_src));

    if let Some(h) = old_home {
        std::env::set_var("HOME", h);
    } else {
        std::env::remove_var("HOME");
    }
}
