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
    let stdout = "cargo:rerun-if-changed=build.rs\ncargo:rerun-if-changed=build.rs\ncargo:rustc-cfg=feat\n";
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
    assert!(!dir.is_fresh(temp.path()));
    assert!(dir.is_fresh_with_missing(temp.path(), true));

    // If the file suddenly appears, freshness must become false
    fs::write(temp.path().join("nonexistent_header.h"), "now exists").unwrap();
    assert!(!dir.is_fresh(temp.path()));
}
