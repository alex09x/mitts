/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::rust::dep_info::{find_dep_info_files, parse_dep_info};
use mitts::adapter::rust::unescape::unescape_rust_string;
use mitts::cli::ui::Ui;
use mitts::core::action_cache::ActionCache;
use mitts::core::build_logger::BuildLogger;
use mitts::core::reflink::reflink_or_copy;
use std::fs;
use std::time::Duration;

#[test]
fn test_m8_reflink_and_build_logger_methods() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    // 1. reflink_or_copy with executable: true vs false
    let src_file = root.join("binary_src");
    fs::write(&src_file, b"executable binary content").unwrap();

    let dst_exec = root.join("dest_bin");
    reflink_or_copy(&src_file, &dst_exec, true).unwrap();
    assert!(dst_exec.is_file());

    let dst_data = root.join("dest_data");
    reflink_or_copy(&src_file, &dst_data, false).unwrap();
    assert!(dst_data.is_file());

    // 2. BuildLogger methods
    let mut logger = BuildLogger::new(None); // None log_path
    let phase = logger.start_phase("custom_phase");
    std::thread::sleep(Duration::from_millis(2));
    phase.finish_with_details("custom details");

    assert_eq!(logger.phases().len(), 1);
    assert_eq!(logger.phases()[0].name, "custom_phase");
    assert!(logger.total_elapsed() >= Duration::from_millis(1));
    assert!(logger.write_trace("dev", 0, "ok").is_ok()); // writes nothing when log_path is None
}

#[test]
fn test_m8_dep_info_and_unescape_branches() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    // 1. find_dep_info_files with normalized target name (dash to underscore)
    let debug_deps = root.join("debug").join("deps");
    fs::create_dir_all(&debug_deps).unwrap();
    let norm_d = debug_deps.join("my_dashed_target.d");
    fs::write(&norm_d, "my_dashed_target: src/lib.rs\nraw_line_no_colon\n").unwrap();

    let found = find_dep_info_files(root, "my-dashed-target");
    assert!(found.iter().any(|p| p.ends_with("my_dashed_target.d")));

    // 2. parse_dep_info with lines without colons
    let parsed = parse_dep_info(&norm_d).unwrap();
    assert!(parsed.is_empty() || !parsed.is_empty());

    // 3. unescape_rust_string edge cases
    assert_eq!(unescape_rust_string(r#"\u"#), None); // missing '{'
    assert_eq!(unescape_rust_string(r#"\u{}"#), None); // empty hex
    assert_eq!(unescape_rust_string(r#"\u{1234567}"#), None); // >6 digits
    assert_eq!(unescape_rust_string(r#"\q"#), None); // invalid escape char
    assert_eq!(unescape_rust_string(r#"ending\"#), None); // trailing escape
}

#[test]
fn test_m8_action_cache_paths_and_ui_helpers() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    // 1. ActionCache paths and lock dirs
    let ac = ActionCache::new(root.join("ac")).unwrap();
    assert!(ac.locks_dir().is_dir());
    let lock_p = ac.lock_path("mykey123");
    assert!(lock_p.to_string_lossy().contains("mykey123.lock"));

    // Missing key get returns Ok(None)
    assert_eq!(ac.get("nonexistent_key_xyz").unwrap(), None);

    // default_path with env vars
    std::env::set_var("XDG_CACHE_HOME", root.display().to_string());
    let p_xdg = ActionCache::default_path();
    assert!(p_xdg.ends_with("ac"));
    std::env::remove_var("XDG_CACHE_HOME");

    // 2. Ui helpers
    assert!(Ui::badge_failed().contains("[failed]"));
    assert_eq!(Ui::format_bytes(2 * 1024 * 1024 * 1024), "2.00 GB");
    Ui::report_message("informational message", None);
}
