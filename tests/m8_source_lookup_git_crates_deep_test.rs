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
use std::sync::Mutex;
use tempfile::TempDir;

static ENV_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn test_m8_source_lookup_git_and_cargo_flow() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let temp = TempDir::new().unwrap();
    let home = temp.path();
    let old_home = std::env::var_os("HOME");
    let old_ch = std::env::var_os("CARGO_HOME");
    std::env::remove_var("CARGO_HOME");
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

    let found_mitts = find_crate_source_dir(
        "tokio",
        "1.30.0",
        Some("registry+https://github.com/rust-lang/crates.io-index"),
    );
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

    if let Some(ch) = old_ch {
        std::env::set_var("CARGO_HOME", ch);
    }
    if let Some(h) = old_home {
        std::env::set_var("HOME", h);
    }
}

#[test]
fn test_t0_g1_git_checkouts_ignores_non_dir_files_and_verifies_package_name_version() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let temp = TempDir::new().unwrap();
    let home = temp.path();
    let old_home = std::env::var_os("HOME");
    let old_ch = std::env::var_os("CARGO_HOME");
    std::env::remove_var("CARGO_HOME");
    std::env::set_var("HOME", home);

    let checkouts = home.join(".cargo").join("git").join("checkouts");
    fs::create_dir_all(&checkouts).unwrap();
    // Non-directory files in git/checkouts (e.g. .DS_Store) must not abort iteration
    fs::write(checkouts.join(".DS_Store"), b"junk").unwrap();
    fs::write(checkouts.join("0000_non_dir_entry"), b"junk").unwrap();

    let commit_dir = checkouts.join("zzzz-repo-9999").join("a1b2c3d4e5f6");
    fs::create_dir_all(&commit_dir).unwrap();

    // Sibling crate with [[bin]] name = "target_pkg" (must NOT match package name "target_pkg")
    let decoy_bin = commit_dir.join("decoy_bin");
    fs::create_dir_all(&decoy_bin).unwrap();
    fs::write(
        decoy_bin.join("Cargo.toml"),
        "[package]\nname = \"other_pkg\"\nversion = \"0.2.0\"\n\n[[bin]]\nname = \"target_pkg\"\npath = \"src/main.rs\"\n",
    )
    .unwrap();

    // Real package with version 0.2.0
    let real_pkg = commit_dir.join("real_pkg");
    fs::create_dir_all(&real_pkg).unwrap();
    fs::write(
        real_pkg.join("Cargo.toml"),
        "[package]\nname = \"target_pkg\"\nversion = \"0.2.0\"\n",
    )
    .unwrap();

    let git_src = "git+https://github.com/example/repo#a1b2c3d4e5f6";
    assert_eq!(
        find_crate_source_dir("target_pkg", "0.2.0", Some(git_src)),
        Some(real_pkg)
    );
    assert_eq!(
        find_crate_source_dir("target_pkg", "0.9.9", Some(git_src)),
        None,
        "mismatched package.version in git checkout must not match"
    );

    if let Some(ch) = old_ch {
        std::env::set_var("CARGO_HOME", ch);
    }
    if let Some(h) = old_home {
        std::env::set_var("HOME", h);
    }
}
