/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::rust::build_script::BuildScriptDirectives;

#[test]
fn test_m8_build_script_directives_parsing_modern_and_legacy() {
    let stdout = r#"
cargo::rustc-cfg=has_neon
cargo:rustc-cfg=feature="simd"
cargo::rustc-link-lib=static=crypto
cargo:rustc-link-lib=dylib=z
cargo::rustc-link-search=native=/usr/local/lib
cargo:rustc-link-search=/opt/homebrew/lib
cargo::rustc-env=CUSTOM_KEY=custom_value
cargo:rustc-env=ANOTHER_KEY=123
cargo::custom_prop=prop_value
cargo:metadata=root_dir=/opt/assets
cargo::rerun-if-changed=build.rs
cargo:warning=Compiling external C dependency
cargo:rustc-check-cfg=cfg(has_neon)
cargo:rustc-flags=-l ssl
non-cargo output line that should be completely ignored
"#;

    let dir = BuildScriptDirectives::parse_stdout(stdout);
    assert!(!dir.is_empty());
    assert_eq!(dir.cfgs, vec!["has_neon", "feature=\"simd\""]);
    assert_eq!(dir.link_libs, vec!["static=crypto", "dylib=z"]);
    assert_eq!(
        dir.link_searches,
        vec!["/usr/local/lib", "/opt/homebrew/lib"]
    );
    assert_eq!(dir.envs.get("CUSTOM_KEY").unwrap(), "custom_value");
    assert_eq!(dir.envs.get("ANOTHER_KEY").unwrap(), "123");
    assert_eq!(dir.metadata.get("custom_prop").unwrap(), "prop_value");
    assert_eq!(dir.metadata.get("root_dir").unwrap(), "/opt/assets");
    assert!(!dir.metadata.contains_key("rerun-if-changed"));
    assert!(!dir.metadata.contains_key("warning"));
    assert!(!dir.metadata.contains_key("rustc-flags"));

    let mut dir_with_links = dir;
    dir_with_links.links = Some("foo".to_string());
    let dep_vars = dir_with_links.dep_env_vars();
    assert_eq!(dep_vars.get("DEP_FOO_CUSTOM_PROP").unwrap(), "prop_value");
    assert_eq!(dep_vars.get("DEP_FOO_ROOT_DIR").unwrap(), "/opt/assets");
}

#[test]
fn test_m8_build_script_directives_empty() {
    let dir = BuildScriptDirectives::parse_stdout("");
    assert!(dir.is_empty());
    assert!(dir.dep_env_vars().is_empty());

    let empty_dir = BuildScriptDirectives::default();
    assert!(empty_dir.is_empty());
}
