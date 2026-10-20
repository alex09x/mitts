/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::rust::env_macro::scan_env_macros;
use mitts::adapter::rust::source_lookup::find_crate_source_dir;
use mitts::adapter::rust::types::EnvMacroDiscovery;
use std::fs;
use std::sync::Mutex;
use tempfile::TempDir;

static ENV_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn test_m8_env_macro_parser_deep_edge_cases() {
    // 1. Nested block comments and unclosed comments
    let src_nested = "/* outer /* inner */ outer again */ fn foo() {}";
    assert_eq!(
        scan_env_macros(src_nested),
        EnvMacroDiscovery::Tracked(vec![])
    );

    let src_unclosed_line = "fn foo() {}\n// unclosed line comment without trailing newline";
    assert_eq!(
        scan_env_macros(src_unclosed_line),
        EnvMacroDiscovery::Tracked(vec![])
    );

    let src_unclosed_block = "fn foo() {} /* unclosed block comment";
    assert_eq!(
        scan_env_macros(src_unclosed_block),
        EnvMacroDiscovery::Tracked(vec![])
    );

    // 2. Unclosed string literals and raw string literals in normal source
    let src_unclosed_raw = "fn foo() {} r#\"unclosed raw string";
    assert_eq!(
        scan_env_macros(src_unclosed_raw),
        EnvMacroDiscovery::Tracked(vec![])
    );

    let src_unclosed_str = "fn foo() {} \"unclosed normal string";
    assert_eq!(
        scan_env_macros(src_unclosed_str),
        EnvMacroDiscovery::Tracked(vec![])
    );

    let src_bytes_and_chars = "fn foo() { let _ = b'x'; let _ = 'y'; }";
    assert_eq!(
        scan_env_macros(src_bytes_and_chars),
        EnvMacroDiscovery::Tracked(vec![])
    );

    // 3. env! macro dynamic / untracked arguments
    let src_env_raw_invalid = "fn foo() { env!(r#INVALID); }";
    assert_eq!(
        scan_env_macros(src_env_raw_invalid),
        EnvMacroDiscovery::UntrackedDynamic
    );

    let src_env_raw_unclosed = "fn foo() { env!(r#\"unclosed); }";
    assert_eq!(
        scan_env_macros(src_env_raw_unclosed),
        EnvMacroDiscovery::UntrackedDynamic
    );

    let src_env_non_str = "fn foo() { env!(12345); }";
    assert_eq!(
        scan_env_macros(src_env_non_str),
        EnvMacroDiscovery::UntrackedDynamic
    );

    let src_env_unclosed_parens = "fn foo() { env!(\"MY_VAR\"";
    assert_eq!(
        scan_env_macros(src_env_unclosed_parens),
        EnvMacroDiscovery::Tracked(vec!["MY_VAR".to_string()])
    );

    // 4. Valid raw string env! macro
    let src_env_raw_valid = "fn foo() { env!(r#\"MY_RAW_VAR\"#); }";
    assert_eq!(
        scan_env_macros(src_env_raw_valid),
        EnvMacroDiscovery::Tracked(vec!["MY_RAW_VAR".to_string()])
    );
}

#[test]
fn test_m8_source_lookup_git_and_cache_hierarchies() {
    let _lock = ENV_LOCK.lock().unwrap();
    let temp = TempDir::new().unwrap();
    let home = temp.path();
    let old_home = std::env::var_os("HOME");
    let old_ch = std::env::var_os("CARGO_HOME");
    std::env::remove_var("CARGO_HOME");
    std::env::set_var("HOME", home);
    let orig_xdg_cache_home = std::env::var_os("XDG_CACHE_HOME");
    std::env::remove_var("XDG_CACHE_HOME");

    // 1. Package in ~/.cache/mitts/sources/<name>-<version>
    let mitts_pkg = home
        .join(".cache")
        .join("mitts")
        .join("sources")
        .join("mitts_pkg-0.2.0");
    fs::create_dir_all(&mitts_pkg).unwrap();
    fs::write(
        mitts_pkg.join("Cargo.toml"),
        "[package]\nname = \"mitts_pkg\"\nversion = \"0.2.0\"\n",
    )
    .unwrap();
    fs::write(mitts_pkg.join(".mitts-ok"), "ok\n").unwrap();

    let found_mitts = find_crate_source_dir(
        "mitts_pkg",
        "0.2.0",
        Some("registry+https://github.com/rust-lang/crates.io-index"),
    );
    assert_eq!(found_mitts, Some(mitts_pkg));

    // 2. Git package found via git checkouts with matching commit hash
    let checkouts = home.join(".cargo").join("git").join("checkouts");
    let repo_dir = checkouts.join("fallback-repo-1234");
    let commit_dir = repo_dir.join("feedbeef1234");
    fs::create_dir_all(&commit_dir).unwrap();
    fs::write(
        commit_dir.join("Cargo.toml"),
        "[package]\nname = \"git_fallback_pkg\"\nversion = \"1.0.0\"\n",
    )
    .unwrap();

    let found_fallback = find_crate_source_dir(
        "git_fallback_pkg",
        "1.0.0",
        Some("git+https://github.com/org/fallback-repo#feedbeef1234"),
    );
    assert_eq!(found_fallback, Some(commit_dir));

    // 3. Non-existent package returns None
    let not_found = find_crate_source_dir("totally_unknown_crate", "0.0.1", None);
    assert_eq!(not_found, None);

    if let Some(ch) = old_ch {
        std::env::set_var("CARGO_HOME", ch);
    }
    match orig_xdg_cache_home {
        Some(v) => std::env::set_var("XDG_CACHE_HOME", v),
        None => std::env::remove_var("XDG_CACHE_HOME"),
    }
    if let Some(h) = old_home {
        std::env::set_var("HOME", h);
    }
}
