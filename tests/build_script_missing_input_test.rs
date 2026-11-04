/*
 * Mitts Build — missing build script input regression
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::process::Command;

#[test]
fn missing_input_reruns_once_per_build_and_detects_creation() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname='missing-input'\nversion='0.1.0'\nedition='2021'\n",
    )
    .unwrap();
    std::fs::write(
        root.join("Cargo.lock"),
        "version=4\n[[package]]\nname='missing-input'\nversion='0.1.0'\n",
    )
    .unwrap();
    std::fs::write(root.join("src/lib.rs"), "include!(concat!(env!(\"OUT_DIR\"), \"/value.rs\"));\n#[test] fn value() { assert!(VALUE > 0); }\n").unwrap();
    std::fs::write(root.join("build.rs"), r#"
        fn main() {
            println!("cargo:rerun-if-changed=sentinel");
            println!("cargo:rerun-if-env-changed=TRACKER");
            let tracker = std::path::PathBuf::from(std::env::var_os("TRACKER").unwrap());
            let count: usize = std::fs::read_to_string(&tracker).unwrap_or_default().parse().unwrap_or(0);
            std::fs::write(&tracker, (count + 1).to_string()).unwrap();
            let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
            std::fs::write(out.join("value.rs"), format!("pub const VALUE: usize = {};", count + 1)).unwrap();
        }
    "#).unwrap();
    let tracker = temp.path().join("counter");
    for expected in 1..=3 {
        if expected == 3 {
            std::fs::write(root.join("sentinel"), "created").unwrap();
        }
        let output = Command::new(env!("CARGO_BIN_EXE_mitts"))
            .current_dir(&root)
            .args(["test", "--lib", "--locked", "--cache-dir"])
            .arg(temp.path().join("cache"))
            .arg("--target-dir")
            .arg(temp.path().join("target"))
            .env("TRACKER", &tracker)
            .env("RUSTC_WRAPPER", "")
            .env("RUSTC_WORKSPACE_WRAPPER", "")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            std::fs::read_to_string(&tracker).unwrap(),
            expected.to_string()
        );
    }
}
