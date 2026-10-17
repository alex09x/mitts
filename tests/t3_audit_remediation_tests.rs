/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::rust::compiler::RustcInvocation;
use mitts::adapter::rust::compiler_args::build_compiler_command;
use mitts::adapter::rust::types::{BuildMode, PlannedCompilationUnit};
use mitts::cli::run::handle_run;
use mitts::cli::test::handle_test;
use mitts::cli::{BuildArgs, RunArgs, TestArgs};
use std::fs;
use tempfile::TempDir;

#[test]
fn test_t3_b1_nb1_nb2_cargo_bin_exe_hyphen_compile_and_runtime_plus_tmpdir_and_warm_cache() {
    let temp = TempDir::new().expect("tempdir");
    let root = temp.path();
    fs::create_dir_all(root.join("src/bin")).unwrap();
    fs::create_dir_all(root.join("tests")).unwrap();

    fs::write(
        root.join("Cargo.toml"),
        r#"[package]
name = "my-cli-pkg"
version = "0.1.0"
edition = "2021"

[features]
extra_cli = []

[[bin]]
name = "my-tool"
path = "src/bin/my-tool.rs"

[[bin]]
name = "feat-tool"
path = "src/bin/feat-tool.rs"
required-features = ["extra_cli"]
"#,
    )
    .unwrap();

    fs::write(
        root.join("src/bin/my-tool.rs"),
        r#"fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(|s| s.as_str()) == Some("--ping") {
        println!("pong");
    }
}
"#,
    )
    .unwrap();

    fs::write(
        root.join("src/bin/feat-tool.rs"),
        "fn main() { println!(\"feat\"); }\n",
    )
    .unwrap();

    fs::write(
        root.join("tests/it_bin.rs"),
        r#"#[test]
fn test_bin_exe_and_tmpdir() {
    let compile_bin = env!("CARGO_BIN_EXE_my-tool");
    let runtime_bin = std::env::var("CARGO_BIN_EXE_my-tool").expect("runtime CARGO_BIN_EXE_my-tool");
    assert_eq!(compile_bin, runtime_bin);

    let out = std::process::Command::new(compile_bin)
        .arg("--ping")
        .output()
        .expect("spawn my-tool");
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "pong");

    // Unbuilt required-features binary must not be exposed
    assert!(option_env!("CARGO_BIN_EXE_feat-tool").is_none());
    assert!(std::env::var("CARGO_BIN_EXE_feat-tool").is_err());

    // CARGO_TARGET_TMPDIR must be available at both compile time and runtime
    let compile_tmp = env!("CARGO_TARGET_TMPDIR");
    let runtime_tmp = std::env::var("CARGO_TARGET_TMPDIR").expect("runtime CARGO_TARGET_TMPDIR");
    assert_eq!(compile_tmp, runtime_tmp);
    let probe = std::path::Path::new(compile_tmp).join("probe.txt");
    std::fs::write(&probe, "ok").expect("write to CARGO_TARGET_TMPDIR");
    assert_eq!(std::fs::read_to_string(&probe).unwrap(), "ok");
}
"#,
    )
    .unwrap();

    let target_dir = root.join("target");
    let cache_dir = root.join("cache");
    let test_args = TestArgs {
        manifest_path: Some(root.join("Cargo.toml")),
        target_dir: Some(target_dir.clone()),
        cache_dir: Some(cache_dir.clone()),
        ..Default::default()
    };

    // Cold build + test run
    assert_eq!(handle_test(&test_args), 0);

    // Warm cache run after removing target_dir
    fs::remove_dir_all(&target_dir).unwrap();
    assert_eq!(handle_test(&test_args), 0);
}

#[test]
fn test_t3_b2_run_multi_binary_disambiguation_and_default_run() {
    let temp = TempDir::new().expect("tempdir");
    let root = temp.path();
    fs::create_dir_all(root.join("src/bin")).unwrap();

    let manifest_path = root.join("Cargo.toml");
    fs::write(
        &manifest_path,
        r#"[package]
name = "multi-bin-pkg"
version = "0.1.0"
edition = "2021"
"#,
    )
    .unwrap();

    // alpha has a compile_error! so if mitts tries to compile all binaries when --bin beta is selected, it would fail!
    fs::write(
        root.join("src/bin/alpha.rs"),
        "compile_error!(\"alpha must not be compiled when beta is selected\");\n",
    )
    .unwrap();

    fs::write(
        root.join("src/bin/beta.rs"),
        r#"fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    std::fs::write(std::path::Path::new(&manifest_dir).join("beta_ran.txt"), "yes").unwrap();
}
"#,
    )
    .unwrap();

    let target_dir = root.join("target");
    let cache_dir = root.join("cache");

    // 1. Ambiguous `mitts run` without --bin or default-run must fail with exit code 101 before compiling alpha
    let ambiguous_args = RunArgs {
        build_args: BuildArgs {
            manifest_path: Some(manifest_path.clone()),
            target_dir: Some(target_dir.clone()),
            cache_dir: Some(cache_dir.clone()),
            ..Default::default()
        },
        trailing_args: Vec::new(),
    };
    assert_eq!(handle_run(&ambiguous_args), 101);

    // 2. Unknown --bin must fail with exit code 101
    let unknown_bin_args = RunArgs {
        build_args: BuildArgs {
            manifest_path: Some(manifest_path.clone()),
            target_dir: Some(target_dir.clone()),
            cache_dir: Some(cache_dir.clone()),
            bin: vec!["nonexistent".to_string()],
            ..Default::default()
        },
        trailing_args: Vec::new(),
    };
    assert_eq!(handle_run(&unknown_bin_args), 101);

    // 3. Explicit `--bin beta` compiles ONLY beta and runs it
    let beta_args = RunArgs {
        build_args: BuildArgs {
            manifest_path: Some(manifest_path.clone()),
            target_dir: Some(target_dir.clone()),
            cache_dir: Some(cache_dir.clone()),
            bin: vec!["beta".to_string()],
            ..Default::default()
        },
        trailing_args: Vec::new(),
    };
    assert_eq!(handle_run(&beta_args), 0);
    assert_eq!(
        fs::read_to_string(root.join("beta_ran.txt")).unwrap(),
        "yes"
    );

    // 4. Setting default-run = "beta" allows `mitts run` without --bin to select and run beta
    fs::remove_file(root.join("beta_ran.txt")).unwrap();
    fs::write(
        &manifest_path,
        r#"[package]
name = "multi-bin-pkg"
version = "0.1.0"
edition = "2021"
default-run = "beta"
"#,
    )
    .unwrap();
    assert_eq!(handle_run(&ambiguous_args), 0);
    assert_eq!(
        fs::read_to_string(root.join("beta_ran.txt")).unwrap(),
        "yes"
    );

    // 5. Warm fast-lock hit still resolves and runs beta
    fs::remove_file(root.join("beta_ran.txt")).unwrap();
    assert_eq!(handle_run(&ambiguous_args), 0);
    assert_eq!(
        fs::read_to_string(root.join("beta_ran.txt")).unwrap(),
        "yes"
    );
}

#[test]
fn test_t3_b3_nb3_nb4_runtime_pkg_env_and_dylib_search_path() {
    let temp = TempDir::new().expect("tempdir");
    let root = temp.path();
    fs::create_dir_all(root.join("src")).unwrap();

    let manifest_path = root.join("Cargo.toml");
    fs::write(
        &manifest_path,
        r#"[package]
name = "env-check-pkg"
version = "1.2.3-alpha.1"
edition = "2021"
authors = ["Alex <alex@prod.codes>"]
description = "Test package metadata"
homepage = "https://prod.codes"
repository = "https://github.com/alex09x/mitts"
license = "MIT OR Apache-2.0"
rust-version = "1.75"
"#,
    )
    .unwrap();

    let check_body = r#"
    assert_eq!(std::env::var("CARGO_PKG_NAME").unwrap(), "env-check-pkg");
    assert_eq!(std::env::var("CARGO_PKG_VERSION").unwrap(), "1.2.3-alpha.1");
    assert_eq!(std::env::var("CARGO_PKG_VERSION_MAJOR").unwrap(), "1");
    assert_eq!(std::env::var("CARGO_PKG_VERSION_MINOR").unwrap(), "2");
    assert_eq!(std::env::var("CARGO_PKG_VERSION_PATCH").unwrap(), "3");
    assert_eq!(std::env::var("CARGO_PKG_VERSION_PRE").unwrap(), "alpha.1");
    assert_eq!(std::env::var("CARGO_PKG_AUTHORS").unwrap(), "Alex <alex@prod.codes>");
    assert_eq!(std::env::var("CARGO_PKG_DESCRIPTION").unwrap(), "Test package metadata");
    assert_eq!(std::env::var("CARGO_PKG_HOMEPAGE").unwrap(), "https://prod.codes");
    assert_eq!(std::env::var("CARGO_PKG_REPOSITORY").unwrap(), "https://github.com/alex09x/mitts");
    assert_eq!(std::env::var("CARGO_PKG_LICENSE").unwrap(), "MIT OR Apache-2.0");
    assert_eq!(std::env::var("CARGO_PKG_RUST_VERSION").unwrap(), "1.75");
    assert!(!std::env::var("CARGO_MANIFEST_DIR").unwrap().is_empty());
    assert!(!std::env::var("CARGO").unwrap().is_empty());
    let ld_path = std::env::var("LD_LIBRARY_PATH").unwrap();
    assert!(ld_path.contains("deps"), "LD_LIBRARY_PATH missing deps: {}", ld_path);
    let dyld_path = std::env::var("DYLD_FALLBACK_LIBRARY_PATH").unwrap();
    assert!(dyld_path.contains("deps"), "DYLD_FALLBACK_LIBRARY_PATH missing deps: {}", dyld_path);
"#;

    fs::write(
        root.join("src/main.rs"),
        format!(
            "fn main() {{{}}}\n#[test]\nfn test_env() {{{}}}\n",
            check_body, check_body
        ),
    )
    .unwrap();

    let target_dir = root.join("target");
    let cache_dir = root.join("cache");

    let run_args = RunArgs {
        build_args: BuildArgs {
            manifest_path: Some(manifest_path.clone()),
            target_dir: Some(target_dir.clone()),
            cache_dir: Some(cache_dir.clone()),
            ..Default::default()
        },
        trailing_args: Vec::new(),
    };
    assert_eq!(handle_run(&run_args), 0);

    let test_args = TestArgs {
        manifest_path: Some(manifest_path),
        target_dir: Some(target_dir),
        cache_dir: Some(cache_dir),
        ..Default::default()
    };
    assert_eq!(handle_test(&test_args), 0);
}

#[test]
fn test_t3_b4_nb5_coverage_target_selection_no_run_and_host_proc_macro_exclusion() {
    let temp = TempDir::new().expect("tempdir");
    let root = temp.path();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(root.join("tests")).unwrap();

    let manifest_path = root.join("Cargo.toml");
    fs::write(
        &manifest_path,
        r#"[package]
name = "cov-select-pkg"
version = "0.1.0"
edition = "2021"
"#,
    )
    .unwrap();

    fs::write(
        root.join("src/lib.rs"),
        "pub fn add(a: i32, b: i32) -> i32 { a + b }\n",
    )
    .unwrap();
    fs::write(
        root.join("tests/it_good.rs"),
        "#[test]\nfn good() { assert_eq!(cov_select_pkg::add(1, 2), 3); }\n",
    )
    .unwrap();
    // If `--test it_good` is not forwarded to coverage, compiling `it_broken` will fail!
    fs::write(
        root.join("tests/it_broken.rs"),
        "compile_error!(\"it_broken must not be compiled when --test it_good is passed\");\n",
    )
    .unwrap();

    let test_args = TestArgs {
        manifest_path: Some(manifest_path),
        target_dir: Some(root.join("target")),
        cache_dir: Some(root.join("cache")),
        coverage: true,
        no_run: true,
        test: vec!["it_good".to_string()],
        ..Default::default()
    };
    assert_eq!(handle_test(&test_args), 0);

    // Also verify NB5a: proc-macro / host units strip `-C instrument-coverage`
    let extra = vec!["-C".to_string(), "instrument-coverage".to_string()];
    let out_dir = root.join("target/mitts/deps");
    let mut proc_macro_unit = PlannedCompilationUnit {
        package_name: "my_macro".to_string(),
        version: "0.1.0".to_string(),
        target_name: "my_macro".to_string(),
        kind: "proc-macro".to_string(),
        edition: Some("2021".to_string()),
        src_path: root.join("src/lib.rs"),
        manifest_path: root.join("Cargo.toml"),
        for_host: true,
        ..Default::default()
    };
    let pm_inv = RustcInvocation {
        unit: &proc_macro_unit,
        action_key: "pm_key",
        out_dir: &out_dir,
        dest_binary: None,
        extern_crates: &[],
        release: false,
        features: &[],
        directives: None,
        mode: BuildMode::Test,
        extra_compiler_args: &extra,
        jobserver_auth: None,
        deps_dir: None,
        target_dir: None,
    };
    let (pm_cmd, _, _) = build_compiler_command(&pm_inv).expect("pm cmd");
    let pm_args: Vec<String> = pm_cmd
        .get_args()
        .map(|a| a.to_string_lossy().to_string())
        .collect();
    assert!(
        !pm_args
            .windows(2)
            .any(|w| w == ["-C", "instrument-coverage"]),
        "proc-macro unit must not receive -C instrument-coverage: {:?}",
        pm_args
    );

    proc_macro_unit.kind = "lib".to_string();
    proc_macro_unit.for_host = false;
    let lib_inv = RustcInvocation {
        unit: &proc_macro_unit,
        action_key: "lib_key",
        out_dir: &out_dir,
        dest_binary: None,
        extern_crates: &[],
        release: false,
        features: &[],
        directives: None,
        mode: BuildMode::Test,
        extra_compiler_args: &extra,
        jobserver_auth: None,
        deps_dir: None,
        target_dir: None,
    };
    let (lib_cmd, _, _) = build_compiler_command(&lib_inv).expect("lib cmd");
    let lib_args: Vec<String> = lib_cmd
        .get_args()
        .map(|a| a.to_string_lossy().to_string())
        .collect();
    assert!(
        lib_args
            .windows(2)
            .any(|w| w == ["-C", "instrument-coverage"]),
        "target lib unit must receive -C instrument-coverage: {:?}",
        lib_args
    );
}
