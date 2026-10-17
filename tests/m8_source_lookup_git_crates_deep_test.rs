/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::rust::source_lookup::find_crate_source_dir;
use std::fs;
use tempfile::TempDir;

#[test]
fn test_m8_source_lookup_git_and_cargo_flow() {
    let temp = TempDir::new().unwrap();
    let home = temp.path();
    let old_home = std::env::var_os("HOME");
    std::env::set_var("HOME", home);

    let checkouts = home.join(".cargo").join("git").join("checkouts");
    let repo_dir = checkouts.join("my-repo-1234");
    let commit_dir = repo_dir.join("a1b2c3d4e5f6");
    fs::create_dir_all(&commit_dir).unwrap();

    // 1. Ignored subdirectories: .hidden and target
    fs::create_dir_all(commit_dir.join(".git")).unwrap();
    fs::create_dir_all(commit_dir.join("target")).unwrap();

    // 2. Direct subdirectory package: sub_pkg
    let sub_pkg = commit_dir.join("sub_pkg");
    fs::create_dir_all(&sub_pkg).unwrap();
    fs::write(
        sub_pkg.join("Cargo.toml"),
        "[package]\nname = \"sub_pkg\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();

    // 3. Nested inside crates/ directory: nested_pkg
    let crates_dir = commit_dir.join("crates").join("nested_pkg");
    fs::create_dir_all(&crates_dir).unwrap();
    fs::write(
        crates_dir.join("Cargo.toml"),
        "[package]\nname = \"nested_pkg\"\nversion = \"0.2.0\"\n",
    )
    .unwrap();

    // Find direct sub_pkg
    let source_url = "git+https://github.com/foo/bar?branch=main#a1b2c3d4";
    let found_sub = find_crate_source_dir("sub_pkg", "0.1.0", Some(source_url));
    assert_eq!(found_sub, Some(sub_pkg));

    // Find nested_pkg under crates/
    let found_nested = find_crate_source_dir("nested_pkg", "0.2.0", Some(source_url));
    assert_eq!(found_nested, Some(crates_dir));

    // 4. Root commit package
    let root_repo = checkouts.join("root-repo-5678");
    let root_commit = root_repo.join("c0ffee012345");
    fs::create_dir_all(&root_commit).unwrap();
    fs::write(
        root_commit.join("Cargo.toml"),
        "[package]\nname = \"root_pkg\"\nversion = \"1.0.0\"\n",
    )
    .unwrap();

    let root_source = "git+https://github.com/foo/root#c0ffee";
    let found_root = find_crate_source_dir("root_pkg", "1.0.0", Some(root_source));
    assert_eq!(found_root, Some(root_commit.clone()));

    // Non-git source must NOT fall back to git checkouts
    let found_fallback = find_crate_source_dir("root_pkg", "1.0.0", None);
    assert_eq!(found_fallback, None);

    // Not found when commit hash mismatch
    let bad_hash_source = "git+https://github.com/foo/bar?branch=main#ffffffff";
    let found_bad = find_crate_source_dir("sub_pkg", "0.1.0", Some(bad_hash_source));
    assert!(found_bad.is_none());

    // 5. Cargo registry source lookup (requires .cargo-ok)
    let reg_dir = home
        .join(".cargo")
        .join("registry")
        .join("src")
        .join("index.crates.io-6f17d22bba15001f")
        .join("serde_json-1.0.100");
    fs::create_dir_all(&reg_dir).unwrap();
    fs::write(
        reg_dir.join("Cargo.toml"),
        "[package]\nname = \"serde_json\"\n",
    )
    .unwrap();
    fs::write(reg_dir.join(".cargo-ok"), "").unwrap();

    let found_reg = find_crate_source_dir(
        "serde_json",
        "1.0.100",
        Some("registry+https://github.com/rust-lang/crates.io-index"),
    );
    assert_eq!(found_reg, Some(reg_dir));

    // 6. Mitts cache source lookup (requires .mitts-ok)
    let mitts_dir = home
        .join(".cache")
        .join("mitts")
        .join("sources")
        .join("tokio-1.30.0");
    fs::create_dir_all(&mitts_dir).unwrap();
    fs::write(
        mitts_dir.join("Cargo.toml"),
        "[package]\nname = \"tokio\"\n",
    )
    .unwrap();
    fs::write(mitts_dir.join(".mitts-ok"), "ok\n").unwrap();

    let found_mitts = find_crate_source_dir("tokio", "1.30.0", None);
    assert_eq!(found_mitts, Some(mitts_dir));

    // 7. Missing checkouts directory
    let empty_home = temp.path().join("empty_home");
    fs::create_dir_all(&empty_home).unwrap();
    std::env::set_var("HOME", &empty_home);

    let res = find_crate_source_dir(
        "any_crate",
        "1.0.0",
        Some("git+https://github.com/test/repo"),
    );
    assert!(res.is_none());

    let res_none = find_crate_source_dir("any_crate", "1.0.0", None);
    assert!(res_none.is_none());

    if let Some(h) = old_home {
        std::env::set_var("HOME", h);
    }
}
