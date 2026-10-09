/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::rust::env_macro::{discover_package_env_macros, scan_env_macros};
use mitts::adapter::rust::types::{EnvMacroDiscovery, RustBuildOptions};
use mitts::adapter::RustAdapter;
use std::fs;
use tempfile::TempDir;

#[test]
fn test_m8_env_macro_raw_strings_and_untracked_dynamics() {
    // 1. Raw string literals r#"..."# and multihash r###"..."###
    let src_raw = r####"
        fn main() {
            let a = env!(r#"RAW_VAR_A"#);
            let b = option_env!(r###"RAW_VAR_B"###);
        }
    "####;
    let disc = scan_env_macros(src_raw);
    match disc {
        EnvMacroDiscovery::Tracked(vars) => {
            assert_eq!(vars, vec!["RAW_VAR_A", "RAW_VAR_B"]);
        }
        _ => panic!("Expected tracked raw string env vars, got {:?}", disc),
    }

    // 2. Non-string token inside env! (e.g. constant or identifier) -> UntrackedDynamic
    let src_const = "fn f() { let x = env!(SOME_CONST_VAR); }";
    assert_eq!(
        scan_env_macros(src_const),
        EnvMacroDiscovery::UntrackedDynamic
    );

    // 3. Unclosed raw string -> UntrackedDynamic
    let src_unclosed_raw = "fn f() { let x = env!(r###\"UNCLOSED_RAW\"##); }";
    assert_eq!(
        scan_env_macros(src_unclosed_raw),
        EnvMacroDiscovery::UntrackedDynamic
    );

    // 4. Invalid escape sequence in normal string -> UntrackedDynamic
    let src_bad_escape = r#"fn f() { let x = env!("BAD_\xZZ_ESC"); }"#;
    assert_eq!(
        scan_env_macros(src_bad_escape),
        EnvMacroDiscovery::UntrackedDynamic
    );

    // 5. Unclosed string literal -> UntrackedDynamic
    let src_unclosed_str = "fn f() { let x = env!(\"UNCLOSED_STR); }";
    assert_eq!(
        scan_env_macros(src_unclosed_str),
        EnvMacroDiscovery::UntrackedDynamic
    );
}

#[test]
fn test_m8_discover_package_env_macros_walk_and_skip() {
    let tmp = TempDir::new().unwrap();
    let p = tmp.path();

    // Setup nested folders including skipped dirs: target, .mitts, .git
    let src_dir = p.join("src").join("nested");
    let target_dir = p.join("target");
    let mitts_dir = p.join(".mitts");
    let git_dir = p.join(".git");

    fs::create_dir_all(&src_dir).unwrap();
    fs::create_dir_all(&target_dir).unwrap();
    fs::create_dir_all(&mitts_dir).unwrap();
    fs::create_dir_all(&git_dir).unwrap();

    // Valid tracked file in src/nested
    fs::write(
        src_dir.join("a.rs"),
        "pub fn f() { let _ = env!(\"TRACKED_NESTED_VAR\"); }\n",
    )
    .unwrap();

    // Untracked dynamic files in skipped dirs (must NOT trigger untracked for package)
    fs::write(target_dir.join("t.rs"), "env!(DYNAMIC_TARGET);\n").unwrap();
    fs::write(mitts_dir.join("m.rs"), "env!(DYNAMIC_MITTS);\n").unwrap();
    fs::write(git_dir.join("g.rs"), "env!(DYNAMIC_GIT);\n").unwrap();

    let (disc, vars) = discover_package_env_macros(p);
    assert_eq!(
        disc,
        EnvMacroDiscovery::Tracked(vec!["TRACKED_NESTED_VAR".to_string()])
    );
    assert_eq!(vars, vec!["TRACKED_NESTED_VAR".to_string()]);

    // Now introduce an untracked dynamic file inside src
    fs::write(
        src_dir.join("dynamic.rs"),
        "pub fn dyn_f() { let _ = env!(DYNAMIC_EXPR); }\n",
    )
    .unwrap();

    let (dyn_disc, dyn_vars) = discover_package_env_macros(p);
    assert_eq!(dyn_disc, EnvMacroDiscovery::UntrackedDynamic);
    assert!(dyn_vars.is_empty());
}

#[test]
fn test_m8_dag_unit_build_script_failure_propagation() {
    let tmp = TempDir::new().unwrap();
    let manifest = tmp.path().join("Cargo.toml");
    let src = tmp.path().join("src");
    fs::create_dir_all(&src).unwrap();

    fs::write(
        &manifest,
        "[package]\nname = \"bad-build-script-pkg\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(src.join("lib.rs"), "pub fn ok() {}\n").unwrap();

    // Invalid build.rs with syntax error
    fs::write(
        tmp.path().join("build.rs"),
        "fn main() { THIS IS BROKEN SYNTAX FOR BUILD.RS }\n",
    )
    .unwrap();

    let opt = RustBuildOptions {
        manifest_path: Some(manifest),
        ..Default::default()
    };
    let (_plan, res) = RustAdapter::build(&opt).unwrap();
    assert!(
        !res.success,
        "Broken build.rs must result in failed ActionResult"
    );
    assert_eq!(res.exit_code, 101);
    assert!(
        res.stderr.contains("Failed to compile build.rs")
            || res.stderr.contains("rustc failed for unit")
    );
}
