/*
 * Mitts Build — test harness argument forwarding regression
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::process::Command;

#[test]
fn ignored_flags_after_separator_reach_test_harness() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    std::fs::create_dir_all(root.join("tests")).unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname='pass-through'\nversion='0.1.0'\nedition='2021'\n",
    )
    .unwrap();
    std::fs::write(root.join("src/lib.rs"), "").unwrap();
    std::fs::write(
        root.join("tests/sample.rs"),
        "#[test] fn normal() { println!(\"visible marker\"); }\n#[test] #[ignore] fn slow() {}\n",
    )
    .unwrap();
    for (flag, expected) in [
        ("--ignored", "1 passed; 0 failed; 0 ignored"),
        ("--include-ignored", "2 passed; 0 failed; 0 ignored"),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_mitts"))
            .current_dir(&root)
            .args(["test", "--offline", "--test", "sample", "--cache-dir"])
            .arg(temp.path().join("cache"))
            .arg("--target-dir")
            .arg(temp.path().join("target"))
            .args(["--", flag])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stdout).contains(expected),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
    }
    let run = |tail: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_mitts"))
            .current_dir(&root)
            .args(["test", "--offline", "--test", "sample", "--cache-dir"])
            .arg(temp.path().join("cache"))
            .arg("--target-dir")
            .arg(temp.path().join("target"))
            .arg("--")
            .args(tail)
            .output()
            .unwrap()
    };
    let exact = run(&["--exact", "norm", "--test-threads=1"]);
    assert!(
        exact.status.success(),
        "{}",
        String::from_utf8_lossy(&exact.stderr)
    );
    assert!(
        String::from_utf8_lossy(&exact.stdout)
            .contains("0 passed; 0 failed; 0 ignored; 0 measured; 2 filtered out"),
        "{}",
        String::from_utf8_lossy(&exact.stdout)
    );
    let nocapture = run(&["--exact", "normal", "--nocapture", "--test-threads=1"]);
    assert!(
        nocapture.status.success(),
        "{}",
        String::from_utf8_lossy(&nocapture.stderr)
    );
    assert!(String::from_utf8_lossy(&nocapture.stdout).contains("visible marker"));
}
