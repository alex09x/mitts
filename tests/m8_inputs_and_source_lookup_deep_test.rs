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
use mitts::adapter::rust::source_lookup::find_crate_source_dir;
use mitts::cli::coverage::{handle_coverage, CoverageArgs};
use std::fs;
use std::sync::Mutex;
use tempfile::TempDir;

static ENV_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn test_m8_inputs_workspace_ancestors_and_home_configs() {
    let _guard = ENV_LOCK.lock().unwrap();
    let orig_home = std::env::var("HOME").ok();
    let orig_cargo_home = std::env::var("CARGO_HOME").ok();

    let temp = TempDir::new().unwrap();
    let ws_root = temp.path().join("my_ws");
    let crates_dir = ws_root.join("crates");
    let member_dir = crates_dir.join("core_member");
    fs::create_dir_all(member_dir.join("src")).unwrap();

    // 1. Workspace root manifests and cargo configs
    fs::write(
        ws_root.join("Cargo.toml"),
        br#"[workspace]
members = ["crates/*"]
"#,
    )
    .unwrap();
    fs::write(ws_root.join("Cargo.lock"), b"# lockfile content\n").unwrap();

    let ws_cargo = ws_root.join(".cargo");
    fs::create_dir_all(&ws_cargo).unwrap();
    fs::write(ws_cargo.join("config.toml"), b"# config.toml\n").unwrap();
    fs::write(ws_cargo.join("config"), b"# config\n").unwrap();

    // 2. Intermediate ancestor manifest
    fs::write(
        crates_dir.join("Cargo.toml"),
        br#"[package]
name = "crates_meta"
version = "0.1.0"
edition = "2021"
"#,
    )
    .unwrap();

    // 3. Member package
    fs::write(
        member_dir.join("Cargo.toml"),
        br#"[package]
name = "core_member"
version = "0.2.0"
edition = "2021"
"#,
    )
    .unwrap();
    fs::write(member_dir.join("src").join("lib.rs"), b"pub fn f() {}\n").unwrap();

    // 4. Fake HOME with ~/.cargo/config.toml and ~/.cargo/config
    let fake_home = temp.path().join("user_home");
    let home_cargo = fake_home.join(".cargo");
    fs::create_dir_all(&home_cargo).unwrap();
    fs::write(home_cargo.join("config.toml"), b"# user config toml\n").unwrap();
    fs::write(home_cargo.join("config"), b"# user config\n").unwrap();

    std::env::remove_var("CARGO_HOME");
    std::env::set_var("HOME", &fake_home);

    let ws_manifest = ws_root.join("Cargo.toml");
    let inputs = hash_package_base_inputs(&member_dir, Some(&ws_manifest), true).unwrap();

    assert!(inputs.iter().any(|(k, _)| k == "workspace:Cargo.toml"));
    assert!(inputs.iter().any(|(k, _)| k.starts_with("ancestor:")));
    assert!(inputs.iter().any(|(k, _)| k == "Cargo.lock"));
    assert!(inputs
        .iter()
        .any(|(k, _)| k == "workspace:.cargo/config.toml"));
    assert!(inputs.iter().any(|(k, _)| k == "workspace:.cargo/config"));
    assert!(inputs.iter().any(|(k, _)| k == "~/.cargo/config.toml"));
    assert!(inputs.iter().any(|(k, _)| k == "~/.cargo/config"));

    // 5. Test append_unit_dep_info_inputs tracking extra assets
    let extra_asset = member_dir.join("weights.bin");
    fs::write(&extra_asset, b"weights binary data").unwrap();

    let target_dir = temp.path().join("target");
    let target_dep_dir = target_dir.join("debug").join("deps");
    fs::create_dir_all(&target_dep_dir).unwrap();

    let dep_info_content = format!(
        "{}: {} {}\n",
        target_dep_dir.join("core_member.d").display(),
        member_dir.join("src").join("lib.rs").display(),
        extra_asset.display()
    );
    fs::write(target_dep_dir.join("core_member.d"), dep_info_content).unwrap();

    let appended =
        append_unit_dep_info_inputs(inputs, &member_dir, &target_dir, "core_member").unwrap();
    assert!(appended.iter().any(|(k, _)| k == "weights.bin"));

    if let Some(h) = orig_home {
        std::env::set_var("HOME", h);
    } else {
        std::env::remove_var("HOME");
    }
    if let Some(c) = orig_cargo_home {
        std::env::set_var("CARGO_HOME", c);
    }
}

#[test]
fn test_m8_source_lookup_commit_root_subfolder_and_mitts_cache() {
    let _guard = ENV_LOCK.lock().unwrap();
    let orig_home = std::env::var("HOME").ok();

    let temp = TempDir::new().unwrap();
    let home = temp.path().join("home_src");
    std::env::set_var("HOME", &home);

    let checkouts = home.join(".cargo").join("git").join("checkouts");
    let repo_dir = checkouts.join("my-repo-123");
    let commit_dir = repo_dir.join("abc1234567890def");
    fs::create_dir_all(&commit_dir).unwrap();

    // 1. Direct crate at commit root
    fs::write(
        commit_dir.join("Cargo.toml"),
        br#"[package]
name = "root_git_crate"
version = "0.5.0"
"#,
    )
    .unwrap();

    let found_root = find_crate_source_dir(
        "root_git_crate",
        "0.5.0",
        Some("git+https://github.com/org/repo#abc12345"),
    );
    assert_eq!(found_root, Some(commit_dir.clone()));

    // 2. Subcrate directly in commit directory (e.g. subfolder with Cargo.toml)
    let sub_pkg = commit_dir.join("nested_sub");
    fs::create_dir_all(&sub_pkg).unwrap();
    fs::write(
        sub_pkg.join("Cargo.toml"),
        br#"[package]
name = "nested_sub_crate"
version = "0.5.0"
"#,
    )
    .unwrap();

    let found_sub = find_crate_source_dir(
        "nested_sub_crate",
        "0.5.0",
        Some("git+https://github.com/org/repo#abc12345"),
    );
    assert_eq!(found_sub, Some(sub_pkg));

    // 3. Mitts source store cache hit in ~/.cache/mitts/sources/name-version
    let mitts_pkg = home
        .join(".cache")
        .join("mitts")
        .join("sources")
        .join("mitts_cached_pkg-1.2.0");
    fs::create_dir_all(&mitts_pkg).unwrap();
    fs::write(
        mitts_pkg.join("Cargo.toml"),
        br#"[package]
name = "mitts_cached_pkg"
version = "1.2.0"
"#,
    )
    .unwrap();
    fs::write(mitts_pkg.join(".mitts-ok"), "ok\n").unwrap();

    let found_mitts = find_crate_source_dir("mitts_cached_pkg", "1.2.0", None);
    assert_eq!(found_mitts, Some(mitts_pkg));

    if let Some(h) = orig_home {
        std::env::set_var("HOME", h);
    } else {
        std::env::remove_var("HOME");
    }
}

#[test]
fn test_m8_coverage_test_failure_and_open_html() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = TempDir::new().unwrap();
    let pkg_dir = temp.path().join("failing_test_pkg");
    fs::create_dir_all(pkg_dir.join("tests")).unwrap();

    fs::write(
        pkg_dir.join("Cargo.toml"),
        br#"[package]
name = "failing_test_pkg"
version = "0.1.0"
edition = "2021"

[workspace]
"#,
    )
    .unwrap();

    fs::write(
        pkg_dir.join("tests").join("bad_test.rs"),
        b"#[test]\nfn test_will_fail() {\n    panic!(\"intentional test failure\");\n}\n",
    )
    .unwrap();

    let manifest = pkg_dir.join("Cargo.toml");
    let target_dir = temp.path().join("target");

    // Test binary failure returns nonzero exit code
    let args_fail = CoverageArgs {
        manifest_path: Some(manifest),
        target_dir: Some(target_dir),
        ..Default::default()
    };
    assert_ne!(handle_coverage(&args_fail), 0);
}
