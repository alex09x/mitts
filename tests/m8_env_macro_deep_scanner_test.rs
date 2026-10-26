/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::rust::env_macro::{discover_package_env_macros, scan_env_macros};
use mitts::adapter::rust::types::EnvMacroDiscovery;
use std::fs;
use tempfile::TempDir;

#[test]
fn test_m8_env_macro_delimiters_and_raw_strings() {
    let source = r##"
// Line comment with env!("COMMENTED_OUT")
/* Block comment /* nested block comment */ env!("NESTED_COMMENT") */
fn test_envs() {
    let _ = env!["BRACKET_VAR"];
    let _ = option_env!{"BRACE_VAR"};
    let _ = env!(r#"RAW_HASH_VAR"#);
    let _ = env!(r"RAW_SIMPLE_VAR");
    let _ = my_env!("PREFIXED_IGNORED");
    let b = b"some byte string with env!(\"BYTE_IGNORED\")";
    let br = br#"some raw byte string with env!("RAW_BYTE_IGNORED")"#;
    let _ = env!("ESCAPED_\"VAR\"");
}
"##;

    let res = scan_env_macros(source);
    match res {
        EnvMacroDiscovery::Tracked(vars) => {
            assert!(vars.contains(&"BRACKET_VAR".to_string()));
            assert!(vars.contains(&"BRACE_VAR".to_string()));
            assert!(vars.contains(&"RAW_HASH_VAR".to_string()));
            assert!(vars.contains(&"RAW_SIMPLE_VAR".to_string()));
            assert!(vars.contains(&"ESCAPED_\"VAR\"".to_string()));
            assert!(!vars.contains(&"COMMENTED_OUT".to_string()));
            assert!(!vars.contains(&"NESTED_COMMENT".to_string()));
            assert!(!vars.contains(&"PREFIXED_IGNORED".to_string()));
            assert!(!vars.contains(&"BYTE_IGNORED".to_string()));
            assert!(!vars.contains(&"RAW_BYTE_IGNORED".to_string()));
        }
        EnvMacroDiscovery::UntrackedDynamic => panic!("Expected tracked env vars"),
    }
}

#[test]
fn test_m8_env_macro_dynamic_and_unclosed_literals() {
    // 1. Dynamic non-string argument
    let dynamic_src = r#"
fn test_dynamic() {
    let key = "MY_VAR";
    let _ = env!(key);
}
"#;
    assert_eq!(
        scan_env_macros(dynamic_src),
        EnvMacroDiscovery::UntrackedDynamic
    );

    // 2. Unclosed string inside env!
    let unclosed_str = "fn unclosed() { let _ = env!(\"UNCLOSED_STRING";
    assert_eq!(
        scan_env_macros(unclosed_str),
        EnvMacroDiscovery::UntrackedDynamic
    );

    // 3. Unclosed raw string inside env!
    let unclosed_raw = "fn unclosed_raw() { let _ = env!(r#\"UNCLOSED_RAW";
    assert_eq!(
        scan_env_macros(unclosed_raw),
        EnvMacroDiscovery::UntrackedDynamic
    );
}

#[test]
fn test_m8_scan_package_env_vars_with_dynamic_file() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir).unwrap();

    let lib_rs = src_dir.join("lib.rs");
    fs::write(
        &lib_rs,
        "pub fn foo() { let _ = env!(\"STATIC_VAR\"); let _ = env!(get_dynamic()); }\n",
    )
    .unwrap();

    let (disc, vars) = discover_package_env_macros(root);
    assert_eq!(disc, EnvMacroDiscovery::UntrackedDynamic);
    assert!(vars.is_empty());
}

/// `env!(#lit)` inside `quote!` is a proc macro emitting the call for another crate
/// (clap_derive), not a dynamic read of this crate's environment.
#[test]
fn quote_interpolation_is_not_a_dynamic_env_read() {
    let src = r#"
        fn emit(env_var_lit: &str) -> TokenStream {
            quote! { let _ = ::core::env!(#env_var_lit); }
        }
        const HERE: &str = env!("REAL_VAR");
    "#;
    assert_eq!(
        scan_env_macros(src),
        EnvMacroDiscovery::Tracked(vec!["REAL_VAR".to_string()])
    );
    assert_eq!(
        scan_env_macros("let v = env!(name);"),
        EnvMacroDiscovery::UntrackedDynamic
    );
}
