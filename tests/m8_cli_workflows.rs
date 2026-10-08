/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::cli::{
    handle_build, handle_check, handle_clean, handle_clippy, handle_explain, handle_fetch,
    handle_fmt, handle_gc, handle_run, handle_test, BuildArgs, CheckArgs, CleanArgs, ClippyArgs,
    ExplainArgs, FetchArgs, FmtArgs, GcArgs, RunArgs, TestArgs,
};
use std::path::PathBuf;

fn fixture_manifest() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("hello-rust")
        .join("Cargo.toml")
}

#[test]
fn test_m8_cli_handle_build_and_run() {
    let manifest = fixture_manifest();
    let temp_cache = tempfile::tempdir().expect("cache tempdir");

    let b_args = BuildArgs {
        manifest_path: Some(manifest.clone()),
        cache_dir: Some(temp_cache.path().to_path_buf()),
        native: true,
        ..Default::default()
    };
    let code = handle_build(&b_args);
    assert_eq!(code, 0, "handle_build should exit with 0");

    let r_args = RunArgs {
        build_args: b_args.clone(),
        trailing_args: vec!["arg1".to_string()],
    };
    let r_code = handle_run(&r_args);
    assert_eq!(r_code, 0, "handle_run should exit with 0");
}

#[test]
fn test_m8_cli_handle_check_and_clippy() {
    let manifest = fixture_manifest();
    let temp_cache = tempfile::tempdir().expect("cache tempdir");

    let c_args = CheckArgs {
        manifest_path: Some(manifest.clone()),
        cache_dir: Some(temp_cache.path().to_path_buf()),
        ..Default::default()
    };
    let code = handle_check(&c_args);
    assert_eq!(code, 0, "handle_check should exit with 0");

    let clip_args = ClippyArgs {
        manifest_path: Some(manifest.clone()),
        cache_dir: Some(temp_cache.path().to_path_buf()),
        ..Default::default()
    };
    let clip_code = handle_clippy(&clip_args);
    assert_eq!(clip_code, 0, "handle_clippy should exit with 0");
}

#[test]
fn test_m8_cli_handle_fmt() {
    let manifest = fixture_manifest();

    let f_args = FmtArgs {
        manifest_path: Some(manifest.clone()),
        check: true,
        ..Default::default()
    };
    let code = handle_fmt(&f_args);
    assert_eq!(code, 0, "handle_fmt --check should exit with 0");
}

#[test]
fn test_m8_cli_handle_test_and_filter() {
    let manifest = fixture_manifest();
    let temp_cache = tempfile::tempdir().expect("cache tempdir");

    let t_args = TestArgs {
        manifest_path: Some(manifest.clone()),
        cache_dir: Some(temp_cache.path().to_path_buf()),
        filter: Some("test_answer".to_string()),
        ..Default::default()
    };
    let code = handle_test(&t_args);
    assert_eq!(code, 0, "handle_test should exit with 0");

    let no_run_args = TestArgs {
        manifest_path: Some(manifest.clone()),
        cache_dir: Some(temp_cache.path().to_path_buf()),
        no_run: true,
        ..Default::default()
    };
    let no_run_code = handle_test(&no_run_args);
    assert_eq!(no_run_code, 0, "handle_test --no-run should exit with 0");
}

#[test]
fn test_m8_cli_handle_explain_and_fetch() {
    let manifest = fixture_manifest();
    let temp_cache = tempfile::tempdir().expect("cache tempdir");

    let exp_args = ExplainArgs {
        manifest_path: Some(manifest.clone()),
        ..Default::default()
    };
    let code = handle_explain(&exp_args);
    assert_eq!(code, 0, "handle_explain should exit with 0");

    let fetch_args = FetchArgs {
        manifest_path: Some(manifest.clone()),
        cache_dir: Some(temp_cache.path().to_path_buf()),
        ..Default::default()
    };
    let fetch_code = handle_fetch(&fetch_args);
    assert_eq!(fetch_code, 0, "handle_fetch should exit with 0");
}

#[test]
fn test_m8_cli_handle_gc_and_clean() {
    let manifest = fixture_manifest();
    let temp_cache = tempfile::tempdir().expect("cache tempdir");

    let gc_args = GcArgs {
        cache_dir: Some(temp_cache.path().to_path_buf()),
        max_bytes: Some(1024 * 1024),
        max_age_secs: Some(3600),
    };
    let gc_code = handle_gc(&gc_args);
    assert_eq!(gc_code, 0, "handle_gc should exit with 0");

    let clean_args = CleanArgs {
        manifest_path: Some(manifest.clone()),
        release: false,
        cache: false,
        cache_dir: None,
    };
    let clean_code = handle_clean(&clean_args);
    assert_eq!(clean_code, 0, "handle_clean dev should exit with 0");

    let clean_rel_args = CleanArgs {
        manifest_path: Some(manifest.clone()),
        release: true,
        cache: true,
        cache_dir: Some(temp_cache.path().to_path_buf()),
    };
    let clean_rel_code = handle_clean(&clean_rel_args);
    assert_eq!(clean_rel_code, 0, "handle_clean release should exit with 0");
}

#[test]
fn test_m8_cli_warm_and_verbose_runs() {
    let manifest = fixture_manifest();
    let temp_cache = tempfile::tempdir().expect("cache tempdir");
    let temp_target = tempfile::tempdir().expect("target tempdir");

    // 1. Build verbose and warm
    let b_args = BuildArgs {
        manifest_path: Some(manifest.clone()),
        cache_dir: Some(temp_cache.path().to_path_buf()),
        target_dir: Some(temp_target.path().to_path_buf()),
        verbose: 1,
        native: true,
        ..Default::default()
    };
    assert_eq!(handle_build(&b_args), 0);
    // Warm build -> Action Cache hit
    assert_eq!(handle_build(&b_args), 0);

    // 2. Check verbose and warm
    let c_args = CheckArgs {
        manifest_path: Some(manifest.clone()),
        cache_dir: Some(temp_cache.path().to_path_buf()),
        target_dir: Some(temp_target.path().to_path_buf()),
        verbose: 1,
        ..Default::default()
    };
    assert_eq!(handle_check(&c_args), 0);
    assert_eq!(handle_check(&c_args), 0);

    // 3. Clippy verbose and warm
    let clip_args = ClippyArgs {
        manifest_path: Some(manifest.clone()),
        cache_dir: Some(temp_cache.path().to_path_buf()),
        target_dir: Some(temp_target.path().to_path_buf()),
        verbose: 1,
        ..Default::default()
    };
    assert_eq!(handle_clippy(&clip_args), 0);
    assert_eq!(handle_clippy(&clip_args), 0);

    // 4. Test verbose
    let t_args = TestArgs {
        manifest_path: Some(manifest.clone()),
        cache_dir: Some(temp_cache.path().to_path_buf()),
        target_dir: Some(temp_target.path().to_path_buf()),
        verbose: 1,
        ..Default::default()
    };
    assert_eq!(handle_test(&t_args), 0);
}

#[test]
fn test_m8_cli_error_paths() {
    let bad_path = Some(PathBuf::from("/non/existent/Cargo.toml"));

    assert_eq!(
        handle_build(&BuildArgs {
            manifest_path: bad_path.clone(),
            ..Default::default()
        }),
        1
    );
    assert_eq!(
        handle_check(&CheckArgs {
            manifest_path: bad_path.clone(),
            ..Default::default()
        }),
        1
    );
    assert_eq!(
        handle_clippy(&ClippyArgs {
            manifest_path: bad_path.clone(),
            ..Default::default()
        }),
        1
    );
    assert_eq!(
        handle_test(&TestArgs {
            manifest_path: bad_path.clone(),
            ..Default::default()
        }),
        1
    );
    assert_eq!(
        handle_run(&RunArgs {
            build_args: BuildArgs {
                manifest_path: bad_path.clone(),
                ..Default::default()
            },
            trailing_args: vec![]
        }),
        1
    );
    assert_eq!(
        handle_explain(&ExplainArgs {
            manifest_path: bad_path.clone(),
            ..Default::default()
        }),
        1
    );
    assert_eq!(
        handle_fetch(&FetchArgs {
            manifest_path: bad_path.clone(),
            ..Default::default()
        }),
        1
    );
    assert_eq!(
        handle_clean(&CleanArgs {
            manifest_path: bad_path.clone(),
            ..Default::default()
        }),
        1
    );

    // Broken crate compilation diagnostics
    let broken = tempfile::tempdir().expect("tempdir");
    let broken_manifest = broken.path().join("Cargo.toml");
    std::fs::create_dir_all(broken.path().join("src")).unwrap();
    std::fs::write(
        &broken_manifest,
        "[package]\nname = \"broken\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(
        broken.path().join("src").join("main.rs"),
        "fn main() { broken!syntax }",
    )
    .unwrap();

    let broken_b = BuildArgs {
        manifest_path: Some(broken_manifest.clone()),
        ..Default::default()
    };
    assert_ne!(handle_build(&broken_b), 0);

    let broken_c = CheckArgs {
        manifest_path: Some(broken_manifest.clone()),
        ..Default::default()
    };
    assert_ne!(handle_check(&broken_c), 0);

    let broken_clip = ClippyArgs {
        manifest_path: Some(broken_manifest.clone()),
        ..Default::default()
    };
    assert_ne!(handle_clippy(&broken_clip), 0);

    let broken_t = TestArgs {
        manifest_path: Some(broken_manifest.clone()),
        ..Default::default()
    };
    assert_ne!(handle_test(&broken_t), 0);
}
