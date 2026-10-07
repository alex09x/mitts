/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::cli::fmt::{handle_fmt, FmtArgs};
use mitts::cli::run::handle_run;
use mitts::cli::{BuildArgs, RunArgs};
use std::fs;
use tempfile::TempDir;

#[test]
fn test_m8_fmt_success_and_check_and_errors() {
    let tmp = TempDir::new().unwrap();
    let pkg_dir = tmp.path().join("fmt-pkg");
    fs::create_dir_all(pkg_dir.join("src")).unwrap();

    let cargo_toml = pkg_dir.join("Cargo.toml");
    fs::write(
        &cargo_toml,
        "[package]\nname = \"fmt-pkg\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();

    let main_rs = pkg_dir.join("src").join("main.rs");
    fs::write(&main_rs, "fn main() { println!(\"unformatted\"); }\n").unwrap();

    // 1. Check formatted
    let args_format = FmtArgs {
        manifest_path: Some(cargo_toml.clone()),
        check: false,
        verbose: 1,
        ..Default::default()
    };
    let code = handle_fmt(&args_format);
    assert_eq!(code, 0);

    // 2. Check mode on formatted code
    let args_check = FmtArgs {
        manifest_path: Some(cargo_toml.clone()),
        check: true,
        ..Default::default()
    };
    let code_chk = handle_fmt(&args_check);
    assert_eq!(code_chk, 0);

    // 3. Intentionally introduce bad formatting
    fs::write(&main_rs, "fn main(){\nprintln!(\"bad\");\n}\n").unwrap();
    let code_chk_bad = handle_fmt(&args_check);
    assert_ne!(code_chk_bad, 0);

    // 4. Missing manifest error
    let args_missing = FmtArgs {
        manifest_path: Some(tmp.path().join("NonExistent.toml")),
        ..Default::default()
    };
    assert_eq!(handle_fmt(&args_missing), 1);

    // 5. Package filter with matching and non-matching names
    let args_matching = FmtArgs {
        manifest_path: Some(cargo_toml.clone()),
        package: Some("fmt-pkg".to_string()),
        ..Default::default()
    };
    assert_eq!(handle_fmt(&args_matching), 0);

    let args_non_matching = FmtArgs {
        manifest_path: Some(cargo_toml.clone()),
        package: Some("different-pkg".to_string()),
        ..Default::default()
    };
    assert_eq!(handle_fmt(&args_non_matching), 0);
}

#[test]
fn test_m8_fmt_no_files_and_invalid_toml() {
    let tmp = TempDir::new().unwrap();
    let empty_dir = tmp.path().join("empty");
    fs::create_dir_all(&empty_dir).unwrap();

    let empty_toml = empty_dir.join("Cargo.toml");
    fs::write(
        &empty_toml,
        "[package]\nname = \"empty-pkg\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();

    let args = FmtArgs {
        manifest_path: Some(empty_toml.clone()),
        ..Default::default()
    };
    assert_eq!(handle_fmt(&args), 0);

    // Corrupted toml syntax
    fs::write(&empty_toml, "invalid [[ toml").unwrap();
    assert_eq!(handle_fmt(&args), 1);
}

#[test]
fn test_m8_run_execution_and_errors() {
    let tmp = TempDir::new().unwrap();
    let bin_dir = tmp.path().join("bin-pkg");
    fs::create_dir_all(bin_dir.join("src")).unwrap();

    let cargo_toml = bin_dir.join("Cargo.toml");
    fs::write(
        &cargo_toml,
        "[package]\nname = \"bin-pkg\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();

    let main_rs = bin_dir.join("src").join("main.rs");
    fs::write(&main_rs, "fn main() { std::process::exit(0); }").unwrap();

    let build_args = BuildArgs {
        manifest_path: Some(cargo_toml.clone()),
        cache_dir: Some(tmp.path().join("cache")),
        target_dir: Some(tmp.path().join("target")),
        ..Default::default()
    };

    let run_args = RunArgs {
        build_args: build_args.clone(),
        trailing_args: vec!["arg1".to_string()],
    };

    let exit_code = handle_run(&run_args);
    assert_eq!(exit_code, 0);

    // 2. Pure library (no binary found)
    let lib_dir = tmp.path().join("lib-pkg");
    fs::create_dir_all(lib_dir.join("src")).unwrap();
    let lib_toml = lib_dir.join("Cargo.toml");
    fs::write(
        &lib_toml,
        "[package]\nname = \"lib-pkg\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(lib_dir.join("src").join("lib.rs"), "pub fn hello() {}").unwrap();

    let lib_run_args = RunArgs {
        build_args: BuildArgs {
            manifest_path: Some(lib_toml),
            cache_dir: Some(tmp.path().join("cache")),
            target_dir: Some(tmp.path().join("target")),
            ..Default::default()
        },
        trailing_args: Vec::new(),
    };
    assert_eq!(handle_run(&lib_run_args), 1);

    // 3. Compilation error
    fs::write(&main_rs, "fn main() { compile_error!(\"fail\"); }").unwrap();
    let fail_run_args = RunArgs {
        build_args,
        trailing_args: Vec::new(),
    };
    assert_ne!(handle_run(&fail_run_args), 0);
}
