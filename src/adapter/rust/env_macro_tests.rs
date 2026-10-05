/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::*;

#[test]
fn test_scan_env_macros_simple() {
    let code = r#"
        fn main() {
            let a = env!("FOO");
            let b = option_env!("BAR");
            let c = env!("BAZ", "error message");
        }
    "#;
    assert_eq!(
        scan_env_macros(code),
        EnvMacroDiscovery::Tracked(vec![
            "BAR".to_string(),
            "BAZ".to_string(),
            "FOO".to_string(),
        ])
    );
}

#[test]
fn test_scan_env_macros_ignores_comments_and_strings() {
    let code = r##"
        // env!("COMMENTED")
        /* option_env!("BLOCK_COMMENTED") */
        /*
           nested /* env!("NESTED") */
        */
        let s = "env!(\"STRING\")";
        let r = r#"env!("RAW_STRING")"#;
        fn main() {
            let actual = env!("REAL_VAR");
        }
    "##;
    assert_eq!(
        scan_env_macros(code),
        EnvMacroDiscovery::Tracked(vec!["REAL_VAR".to_string()])
    );
}

#[test]
fn test_scan_env_macros_detects_untracked_dynamic() {
    let code = r#"
        fn main() {
            let dynamic = env!(concat!("FOO", "_BAR"));
        }
    "#;
    assert_eq!(
        scan_env_macros(code),
        EnvMacroDiscovery::UntrackedDynamic
    );
}

#[test]
fn test_scan_env_macros_escaped_string() {
    let code = r#"
        fn main() {
            let v = env!("BUILD\x5fMODE");
            let u = option_env!("UNICODE\u{5f}NAME");
        }
    "#;
    assert_eq!(
        scan_env_macros(code),
        EnvMacroDiscovery::Tracked(vec![
            "BUILD_MODE".to_string(),
            "UNICODE_NAME".to_string(),
        ])
    );
}

#[test]
fn test_scan_env_macros_unicode_does_not_panic() {
    let code = r#"
        // Line comment with Unicode: 🦀 привет мир!
        /* Block comment with Unicode:
           中文测试, café, résumé, 🚀 emoji, ελληνικά
           nested /* ещё один комментарий */
        */
        fn café() {
            let identifier_ü = 1;
            let real = env!("VAR_AFTER_UNICODE");
        }
    "#;
    assert_eq!(
        scan_env_macros(code),
        EnvMacroDiscovery::Tracked(vec!["VAR_AFTER_UNICODE".to_string()])
    );
}
