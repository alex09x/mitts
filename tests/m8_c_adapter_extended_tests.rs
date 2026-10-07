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
use std::fs;

#[test]
fn test_m8_c_adapter_compilation_error_and_invalid_manifest() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let pkg_dir = temp_dir.path().join("c_error_pkg");
    fs::create_dir_all(&pkg_dir).unwrap();

    let manifest_path = pkg_dir.join("Mitts.toml");
    fs::write(
        &manifest_path,
        "[package]\nname = \"err-c\"\nversion = \"0.1.0\"\n\n[bin]\nsrc = \"bad.c\"\n",
    ).unwrap();

    // C source file with syntax errors
    fs::write(pkg_dir.join("bad.c"), "int main() { this is an invalid syntax error; }\n").unwrap();

    let opt = CBuildOptions {
        manifest_path: Some(manifest_path.clone()),
        target_dir: Some(temp_dir.path().join("target")),
        cache_dir: Some(temp_dir.path().join("cache")),
        release: false,
        no_cache: false,
        extra_flags: vec![],
    };

    let res = CAdapter::build(&opt);
    assert!(res.is_err(), "Invalid C code must cause build error");
    let err_msg = res.unwrap_err().to_string();
    assert!(err_msg.contains("C compilation failed"));

    // Missing package name
    let bad_manifest = pkg_dir.join("bad_Mitts.toml");
    fs::write(&bad_manifest, "[package]\nversion = \"0.1.0\"\n").unwrap();
    let opt_bad = CBuildOptions {
        manifest_path: Some(bad_manifest),
        ..opt.clone()
    };
    assert!(CAdapter::build(&opt_bad).is_err());
}

#[test]
fn test_m8_c_adapter_release_and_no_cache_modes() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let pkg_dir = temp_dir.path().join("c_opt_pkg");
    fs::create_dir_all(&pkg_dir).unwrap();

    let manifest_path = pkg_dir.join("Mitts.toml");
    fs::write(
        &manifest_path,
        "[package]\nname = \"opt-c\"\nversion = \"1.0.0\"\n\n[bin]\nsrc = \"main.c\"\n",
    ).unwrap();

    fs::write(
        pkg_dir.join("main.c"),
        "#include <stdio.h>\nint main() { printf(\"Optimized\\n\"); return 0; }\n",
    ).unwrap();

    // 1. Release mode with extra flags
    let opt_release = CBuildOptions {
        manifest_path: Some(manifest_path.clone()),
        target_dir: Some(temp_dir.path().join("target_release")),
        cache_dir: Some(temp_dir.path().join("cache")),
        release: true,
        no_cache: false,
        extra_flags: vec!["-DOPTIMIZED=1".to_string()],
    };
    let (plan, res) = CAdapter::build(&opt_release).expect("release build");
    assert!(res.success);
    assert!(!res.cached);
    assert_eq!(plan.actions.len(), 1);

    // 2. no_cache mode
    let opt_no_cache = CBuildOptions {
        manifest_path: Some(manifest_path),
        target_dir: Some(temp_dir.path().join("target_no_cache")),
        cache_dir: Some(temp_dir.path().join("cache")),
        release: false,
        no_cache: true,
        extra_flags: vec![],
    };
    let (_, res_no_cache) = CAdapter::build(&opt_no_cache).expect("no cache build");
    assert!(res_no_cache.success);
    assert!(!res_no_cache.cached);
}
