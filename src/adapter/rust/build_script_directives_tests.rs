/*
 * Mitts Build — build script directive tests
 * Copyright (c) 2026 Alexander Panasenko
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */
use super::*;
use std::fs;

#[test]
fn test_parse_stdout_dedup() {
    let stdout =
        "cargo:rerun-if-changed=build.rs\ncargo:rerun-if-changed=build.rs\ncargo:rustc-cfg=feat\n";
    let dir = BuildScriptDirectives::parse_stdout(stdout);
    assert_eq!(dir.rerun_if_changed, vec!["build.rs"]);
    assert_eq!(dir.cfgs, vec!["feat"]);
}

#[test]
fn test_compute_compile_hash_differs_on_cfg() {
    let dir1 = BuildScriptDirectives::parse_stdout("cargo:rustc-cfg=fast\n");
    let dir2 = BuildScriptDirectives::parse_stdout("");
    assert_ne!(dir1.compute_compile_hash(), dir2.compute_compile_hash());
}

#[test]
fn test_compute_compile_hash_normalizes_out_dir() {
    let d1 = BuildScriptDirectives {
        out_dir: Some(PathBuf::from("/tmp/stage_run1")),
        link_searches: vec!["/tmp/stage_run1/lib".to_string(), "/usr/lib".to_string()],
        ..Default::default()
    };

    let d2 = BuildScriptDirectives {
        out_dir: Some(PathBuf::from("/tmp/stage_run2")),
        link_searches: vec!["/tmp/stage_run2/lib".to_string(), "/usr/lib".to_string()],
        ..Default::default()
    };

    assert_eq!(d1.compute_compile_hash(), d2.compute_compile_hash());
}

#[test]
fn test_is_fresh_with_missing_file() {
    let mut dir = BuildScriptDirectives::default();
    dir.rerun_if_changed
        .push("nonexistent_header.h".to_string());
    dir.rerun_if_changed_hashes
        .insert("nonexistent_header.h".to_string(), "<missing>".to_string());
    let temp = tempfile::tempdir().unwrap();
    assert!(!dir.is_fresh(temp.path(), temp.path()));
    assert!(dir.is_fresh_with_missing(temp.path(), temp.path(), true));

    // If the file suddenly appears, freshness must become false
    fs::write(temp.path().join("nonexistent_header.h"), "now exists").unwrap();
    assert!(!dir.is_fresh(temp.path(), temp.path()));
}

/// A variable the script watches is read as the script sees it: `[env]` of the invocation's
/// Cargo config counts, so a changed `[env]` value makes the cached run stale.
#[test]
fn a_watched_variable_from_config_env_makes_the_run_stale_when_it_changes() {
    let var = "MITTS_TEST_WATCHED_CONFIG_ENV_9F3A";
    std::env::remove_var(var);
    let ws = tempfile::tempdir().unwrap();
    fs::create_dir_all(ws.path().join(".cargo")).unwrap();
    fs::write(
        ws.path().join(".cargo/config.toml"),
        format!("[env]\n{var} = \"-DONE\"\n"),
    )
    .unwrap();
    let pkg = tempfile::tempdir().unwrap();
    let mut dir = BuildScriptDirectives::default();
    dir.rerun_if_env_changed.push(var.to_string());
    dir.envs.insert(var.to_string(), "-DONE".to_string());
    assert!(dir.is_fresh(pkg.path(), ws.path()), "same [env] value");
    dir.envs.insert(var.to_string(), "-DOTHER".to_string());
    assert!(!dir.is_fresh(pkg.path(), ws.path()), "another [env] value");
    dir.envs.remove(var);
    assert!(
        !dir.is_fresh(pkg.path(), ws.path()),
        "set in [env], unset in the run"
    );
}

#[test]
fn scoped_link_args_reach_only_their_targets() {
    let dir = BuildScriptDirectives::parse_stdout(
        "cargo:rustc-link-arg=-all\n\
         cargo:rustc-link-arg-bin=rg=/MANIFEST:EMBED\n\
         cargo::rustc-link-arg-bins=-bins\n\
         cargo:rustc-link-arg-tests=-tests\n\
         cargo:rustc-link-arg-benches=-benches\n\
         cargo:rustc-link-arg-examples=-examples\n\
         cargo:rustc-cdylib-link-arg=-cdylib\n",
    );
    assert_eq!(dir.link_args, ["-all"]);
    assert_eq!(
        dir.link_args_for("bin", "rg"),
        ["-all", "/MANIFEST:EMBED", "-bins"]
    );
    assert_eq!(dir.link_args_for("bin", "other"), ["-all", "-bins"]);
    // A binary's unit test is a binary target to Cargo.
    assert_eq!(
        dir.link_args_for("test", "rg_bin_test"),
        ["-all", "/MANIFEST:EMBED", "-bins"]
    );
    assert_eq!(dir.link_args_for("test", "it"), ["-all", "-tests"]);
    assert_eq!(dir.link_args_for("test", "pkg_unit_test"), ["-all"]);
    assert_eq!(dir.link_args_for("bench", "b"), ["-all", "-benches"]);
    assert_eq!(dir.link_args_for("example", "e"), ["-all", "-examples"]);
    assert_eq!(dir.link_args_for("lib,cdylib", "pkg"), ["-all", "-cdylib"]);
    assert_eq!(dir.link_args_for("lib", "pkg"), ["-all"]);
}

#[test]
fn scoped_link_args_change_the_compile_hash() {
    let a = BuildScriptDirectives::parse_stdout("cargo:rustc-link-arg-bin=a=-x\n");
    let b = BuildScriptDirectives::parse_stdout("cargo:rustc-link-arg-bin=b=-x\n");
    assert!(!a.is_empty());
    assert_ne!(a.compute_compile_hash(), b.compute_compile_hash());
}

#[test]
fn out_dir_paths_in_directives_follow_the_out_dir() {
    let mut dir = BuildScriptDirectives::parse_stdout(
        "cargo:rustc-link-arg-bins=/stage/a/out/app.lib\n\
         cargo:rustc-link-search=native=/stage/a/out\n\
         cargo:rustc-link-arg=/elsewhere/x.lib\n\
         cargo:rustc-flags=-L /stage/a/out/sub\n",
    );
    rebase_out_dir(
        &mut dir,
        std::path::Path::new("/stage/a/out"),
        std::path::Path::new("/stable/out"),
    );
    assert_eq!(
        dir.link_args_for("bin", "app"),
        ["/elsewhere/x.lib", "/stable/out/app.lib"]
    );
    assert_eq!(dir.link_searches, ["/stable/out"]);
    assert_eq!(dir.flags, ["-L /stable/out/sub"]);
}
