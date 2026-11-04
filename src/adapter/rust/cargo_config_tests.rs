/*
 * Mitts Build — Cargo configuration tests
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::*;

#[test]
fn test_extract_build_rustflags_string_and_array() {
    let content_str = r#"
[build]
rustflags = "-C opt-level=3 --cfg custom_mark"
"#;
    let val: toml::Value = toml::from_str(content_str).unwrap();
    let extracted =
        extract_rustflags_from_toml(&val, "x86_64-unknown-linux-gnu", Path::new("dummy.toml"))
            .unwrap();
    assert_eq!(
        extracted.build.unwrap(),
        vec!["-C", "opt-level=3", "--cfg", "custom_mark"]
    );

    let content_arr = "[build]\nrustflags = [\"-C\", \"opt-level=2\", \"--cfg\", \"mark_array\"]\n";
    let val2: toml::Value = toml::from_str(content_arr).unwrap();
    let extracted2 =
        extract_rustflags_from_toml(&val2, "x86_64-unknown-linux-gnu", Path::new("dummy.toml"))
            .unwrap();
    assert_eq!(
        extracted2.build.unwrap(),
        vec!["-C", "opt-level=2", "--cfg", "mark_array"]
    );
}

#[test]
fn test_extract_target_triple_rustflags_precedence() {
    let content = "[build]\nrustflags = [\"--cfg\", \"build_flag\"]\n[target.x86_64-unknown-linux-gnu]\nrustflags = [\"--cfg\", \"linux_flag\"]\n";
    let val: toml::Value = toml::from_str(content).unwrap();
    let extracted =
        extract_rustflags_from_toml(&val, "x86_64-unknown-linux-gnu", Path::new("dummy.toml"))
            .unwrap();
    assert_eq!(extracted.target.unwrap(), vec!["--cfg", "linux_flag"]);
    assert_eq!(extracted.build.unwrap(), vec!["--cfg", "build_flag"]);
}

#[test]
fn test_extract_target_cfg_evaluation_and_invalid_syntax() {
    let host = super::super::compiler_resolve::try_resolve_host_triple().unwrap();
    let content = "[target.'cfg(any(unix, windows))']\nrustflags = [\"--cfg\", \"matched_cfg\"]\n[target.'cfg(target_os = \"nonexistent_os\")']\nrustflags = [\"--cfg\", \"unmatched_cfg\"]\n";
    let val: toml::Value = toml::from_str(content).unwrap();
    let extracted = extract_rustflags_from_toml(&val, &host, Path::new("custom.toml")).unwrap();
    assert_eq!(extracted.target.unwrap(), vec!["--cfg", "matched_cfg"]);

    let bad_content = "[target.'cfg(invalid_fn(foo))']\nrustflags = [\"--cfg\", \"bad_cfg\"]\n";
    let bad_val: toml::Value = toml::from_str(bad_content).unwrap();
    assert!(extract_rustflags_from_toml(&bad_val, &host, Path::new("custom.toml")).is_err());
}
