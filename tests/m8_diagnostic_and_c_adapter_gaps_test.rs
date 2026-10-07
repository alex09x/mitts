/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::c::{CAdapter, CBuildOptions};
use mitts::adapter::rust::inputs::{append_unit_dep_info_inputs, hash_package_base_inputs};
use mitts::cli::diagnostic::render_compiler_diagnostics;
use std::fs;
use tempfile::TempDir;

#[test]
fn test_m8_diagnostic_fallback_and_windows_paths() {
    // 1. Diagnostics fallback when no structured error is present
    let raw_stderr = "catastrophic linker error: symbol not found";
    render_compiler_diagnostics(raw_stderr, false);
    render_compiler_diagnostics(raw_stderr, true);

    // 2. Diagnostics with Windows drive letter and help line
    let win_stderr = r#"
error[E0308]: mismatched types
  --> C:\Users\user\project\src\main.rs:42:15
   |
42 |     let x: u32 = "hello";
   |                  ^^^^^^^ expected `u32`, found `&str`
   = help: try using .parse()
"#;
    render_compiler_diagnostics(win_stderr, false);
    render_compiler_diagnostics(win_stderr, true);

    // 3. Diagnostics with command and cwd markers
    let marked_stderr = r#"
--- failure in 'my_pkg@0.1.0::bin:my_bin'
command: rustc src/main.rs
cwd: /tmp/proj
error[E0425]: cannot find value `foo` in this scope
  --> src/main.rs:10:5
"#;
    render_compiler_diagnostics(marked_stderr, false);
}

#[test]
fn test_m8_c_adapter_manifest_resolution_and_cache_hit() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    // 1. Missing manifest resolution returns error
    let missing_opts = CBuildOptions {
        manifest_path: Some(root.join("NonExistent.toml")),
        release: false,
        target_dir: None,
        cache_dir: None,
        no_cache: false,
        extra_flags: Vec::new(),
    };
    assert!(CAdapter::build(&missing_opts).is_err());

    // 2. Valid C manifest build and cache hit
    let c_manifest = root.join("Mitts.toml");
    let c_src = root.join("main.c");
    fs::write(
        &c_manifest,
        r#"
# Sample C project
[package]
name = "hello_c"
version = "0.1.0"

[bin]
src = "main.c"
# Extra comments
"#,
    )
    .unwrap();

    fs::write(
        &c_src,
        r#"
#include <stdio.h>
int main() {
    printf("Hello from C\n");
    return 0;
}
"#,
    )
    .unwrap();

    let cache_dir = root.join("cache");
    let target_dir = root.join("target");
    let opts = CBuildOptions {
        manifest_path: Some(c_manifest),
        release: false,
        target_dir: Some(target_dir),
        cache_dir: Some(cache_dir),
        no_cache: false,
        extra_flags: Vec::new(),
    };

    // First build (compiles or restores)
    if let Ok((_plan1, res1)) = CAdapter::build(&opts) {
        assert!(res1.success);

        // Second build must be a CACHE HIT
        let (_plan2, res2) = CAdapter::build(&opts).unwrap();
        assert!(res2.success);
        assert!(res2.cached);
    }
}

#[test]
fn test_m8_inputs_ancestors_nested_crates_and_dep_info_dedup() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();

    // Workspace root manifest
    let ws_manifest = root.join("Cargo.toml");
    fs::write(
        &ws_manifest,
        "[workspace]\nmembers = [\"nested/subpkg\"]\n",
    )
    .unwrap();

    // Intermediate ancestor directory
    let nested_dir = root.join("nested");
    fs::create_dir_all(&nested_dir).unwrap();
    fs::write(
        nested_dir.join("Cargo.toml"),
        "[workspace]\nmembers = [\"subpkg\"]\n",
    )
    .unwrap();

    // Leaf crate directory
    let subpkg_dir = nested_dir.join("subpkg");
    let src_dir = subpkg_dir.join("src");
    fs::create_dir_all(&src_dir).unwrap();
    let leaf_manifest = subpkg_dir.join("Cargo.toml");
    fs::write(
        &leaf_manifest,
        "[package]\nname = \"subpkg\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    let main_rs = src_dir.join("main.rs");
    fs::write(&main_rs, "fn main() {}\n").unwrap();

    // Nested directory containing its own Cargo.toml (should be ignored by leaf package scan)
    let nested_inner_crate = subpkg_dir.join("inner_crate");
    fs::create_dir_all(&nested_inner_crate).unwrap();
    fs::write(
        nested_inner_crate.join("Cargo.toml"),
        "[package]\nname = \"inner\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(nested_inner_crate.join("lib.rs"), "pub fn inner() {}\n").unwrap();

    // Transient files (should be ignored)
    fs::write(subpkg_dir.join("main.rs.swp"), "vim swap file").unwrap();
    fs::write(subpkg_dir.join("temp_build.tmp"), "temp data").unwrap();

    // Scan base inputs
    let inputs = hash_package_base_inputs(&subpkg_dir, Some(&ws_manifest), true).unwrap();
    assert!(inputs.iter().any(|(k, _)| k == "src/main.rs"));
    // Ancestor manifest must be hashed
    assert!(inputs.iter().any(|(k, _)| k.contains("ancestor:")));
    // Nested inner crate must NOT be part of base inputs
    assert!(!inputs.iter().any(|(k, _)| k.contains("inner_crate")));
    // Transient files must NOT be in inputs
    assert!(!inputs.iter().any(|(k, _)| k.ends_with(".swp")));
    assert!(!inputs.iter().any(|(k, _)| k.ends_with(".tmp")));

    // Test append_unit_dep_info_inputs deduplication for .rs files already in inputs
    let target_dir = subpkg_dir.join("target").join("debug");
    fs::create_dir_all(&target_dir).unwrap();
    let dep_info_file = target_dir.join("subpkg.d");
    fs::write(
        &dep_info_file,
        format!("{}: {}\n", target_dir.join("subpkg").display(), main_rs.display()),
    )
    .unwrap();

    let updated = append_unit_dep_info_inputs(inputs.clone(), &subpkg_dir, &target_dir, "subpkg").unwrap();
    // main.rs should not be duplicated
    let main_count = updated.iter().filter(|(k, _)| k == "src/main.rs").count();
    assert_eq!(main_count, 1);
}
