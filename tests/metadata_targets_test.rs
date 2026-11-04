/*
 * Mitts Build — metadata target discovery regression
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::process::Command;

#[test]
fn metadata_lists_integration_targets_with_manifest_options() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join("tests")).unwrap();
    std::fs::write(root.join("Cargo.toml"), "[package]\nname='demo'\nversion='0.1.0'\nedition='2021'\n[features]\nextra=[]\n[[test]]\nname='special'\nharness=false\nrequired-features=['extra']\n").unwrap();
    std::fs::write(root.join("src/lib.rs"), "pub fn f() {}\n").unwrap();
    std::fs::write(root.join("tests/special.rs"), "fn main() {}\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_mitts"))
        .args(["metadata", "--offline", "--manifest-path"])
        .arg(root.join("Cargo.toml"))
        .arg("--cache-dir")
        .arg(temp.path().join("cache"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let data: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let targets = data["packages"][0]["targets"].as_array().unwrap();
    let target = targets
        .iter()
        .find(|target| target["name"] == "special")
        .unwrap();
    assert_eq!(target["kind"][0], "test");
    assert_eq!(target["harness"], false);
    assert_eq!(target["required-features"][0], "extra");
}
