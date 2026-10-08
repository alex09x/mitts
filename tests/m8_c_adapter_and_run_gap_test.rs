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
use mitts::cli::run::handle_run;
use mitts::cli::RunArgs;
use std::fs;

#[test]
fn test_m8_c_adapter_caching_and_error_paths() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    // 1. Valid C project
    let manifest = root.join("Mitts.toml");
    fs::write(
        &manifest,
        r#"
[package]
name = "c-hello"
version = "1.0.0"

[bin]
src = "main.c"
"#,
    )
    .unwrap();

    let c_src = root.join("main.c");
    fs::write(
        &c_src,
        r#"
#include <stdio.h>
int main() {
    printf("hello from C\n");
    return 0;
}
"#,
    )
    .unwrap();

    let opt = CBuildOptions {
        manifest_path: Some(manifest.clone()),
        cache_dir: Some(root.join(".mitts_cache")),
        target_dir: Some(root.join("target")),
        release: true, // covers -O3
        ..Default::default()
    };

    // 1st build: fresh compilation
    let (plan1, res1) = CAdapter::build(&opt).unwrap();
    assert!(res1.success);
    assert!(!res1.cached);
    assert_eq!(plan1.actions.len(), 1);

    // 2nd build: restore from Action Cache and CAS
    let (plan2, res2) = CAdapter::build(&opt).unwrap();
    assert!(res2.success);
    assert!(
        res2.cached,
        "2nd build should be restored from CAS/AC cache"
    );
    assert_eq!(plan2.actions.len(), 1);

    // 2. Parse errors
    assert!(CAdapter::parse_manifest("[bin]\nsrc=\"main.c\"\n").is_err());
    assert!(CAdapter::parse_manifest("[package]\nname=\"\"\n").is_err());

    // 3. Missing source file error
    let missing_src_opt = CBuildOptions {
        manifest_path: Some(manifest),
        target_dir: Some(root.join("target")),
        ..Default::default()
    };
    let _ = fs::remove_file(&c_src);
    assert!(CAdapter::build(&missing_src_opt).is_err());
}

#[test]
fn test_m8_cli_run_errors_and_missing_binary() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    // 1. Library project without binary targets -> handle_run returns 1
    let manifest = root.join("Cargo.toml");
    fs::write(
        &manifest,
        r#"
[package]
name = "pure-library"
version = "0.1.0"
edition = "2021"
"#,
    )
    .unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src").join("lib.rs"), "pub fn lib_only() {}\n").unwrap();

    let run_args = RunArgs {
        build_args: mitts::cli::BuildArgs {
            manifest_path: Some(manifest.clone()),
            cache_dir: Some(root.join(".mitts_cache")),
            target_dir: Some(root.join("target")),
            ..Default::default()
        },
        trailing_args: Vec::new(),
    };
    assert_eq!(
        handle_run(&run_args),
        1,
        "run should fail on library without binary"
    );

    // 2. Compilation error -> handle_run returns non-zero exit code
    let failing_manifest = root.join("Cargo_fail.toml");
    fs::write(
        &failing_manifest,
        r#"
[package]
name = "fail-bin"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "fail_bin"
path = "src/fail.rs"
"#,
    )
    .unwrap();
    fs::write(
        root.join("src").join("fail.rs"),
        "fn main() { syntax error here }\n",
    )
    .unwrap();

    let fail_run_args = RunArgs {
        build_args: mitts::cli::BuildArgs {
            manifest_path: Some(failing_manifest),
            cache_dir: Some(root.join(".mitts_cache")),
            target_dir: Some(root.join("target")),
            ..Default::default()
        },
        trailing_args: Vec::new(),
    };
    assert_ne!(handle_run(&fail_run_args), 0);

    // 3. Manifest not found -> handle_run returns 1
    let missing_args = RunArgs {
        build_args: mitts::cli::BuildArgs {
            manifest_path: Some(root.join("Nonexistent.toml")),
            ..Default::default()
        },
        trailing_args: Vec::new(),
    };
    assert_eq!(handle_run(&missing_args), 1);
}
