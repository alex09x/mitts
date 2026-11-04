/*
 * Mitts Build — all-targets bench and example regressions
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::process::Command;

#[test]
fn all_targets_checks_examples_and_benches_and_builds_benches_for_test() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    let dev_dep = temp.path().join("dev-dep");
    std::fs::create_dir_all(dev_dep.join("src")).unwrap();
    std::fs::write(
        dev_dep.join("Cargo.toml"),
        "[package]\nname='dev-dep'\nversion='0.1.0'\nedition='2021'\n",
    )
    .unwrap();
    std::fs::write(dev_dep.join("src/lib.rs"), "pub fn f() {}\n").unwrap();
    for dir in ["src", "benches", "examples"] {
        std::fs::create_dir_all(root.join(dir)).unwrap();
    }
    std::fs::write(root.join("Cargo.toml"), format!("[package]\nname='all-targets'\nversion='0.1.0'\nedition='2021'\n[dev-dependencies]\ndev-dep={{path={:?}}}\n[[bench]]\nname='b'\nharness=false\n", dev_dep)).unwrap();
    std::fs::write(root.join("src/lib.rs"), "pub fn f() {}\n").unwrap();
    std::fs::write(
        root.join("benches/b.rs"),
        "const BAD: u8 = \"wrong\"; fn main() {}\n",
    )
    .unwrap();
    std::fs::write(root.join("examples/e.rs"), "fn main() { dev_dep::f(); }\n").unwrap();
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_mitts"))
            .current_dir(&root)
            .args(args)
            .arg("--offline")
            .arg("--cache-dir")
            .arg(temp.path().join("cache"))
            .arg("--target-dir")
            .arg(temp.path().join("target"))
            .output()
            .unwrap()
    };
    let output = run(&["check", "--all-targets"]);
    assert!(
        !output.status.success(),
        "check must detect the broken benchmark"
    );
    for flags in [
        &["test", "--all-targets", "--no-run"][..],
        &["test", "--benches", "--no-run"][..],
    ] {
        let output = run(flags);
        assert!(
            !output.status.success(),
            "test must compile the broken benchmark"
        );
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("mismatched types"),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    std::fs::write(root.join("benches/b.rs"), "fn main() { dev_dep::f(); }\n").unwrap();
    std::fs::write(
        root.join("examples/e.rs"),
        "const BAD: u8 = \"wrong\"; fn main() {}\n",
    )
    .unwrap();
    let output = run(&["check", "--all-targets"]);
    assert!(
        !output.status.success(),
        "check must detect the broken example"
    );
    std::fs::write(root.join("examples/e.rs"), "fn main() { dev_dep::f(); }\n").unwrap();
    std::fs::write(
        root.join("src/main.rs"),
        "const BAD: u8 = \"wrong\"; fn main() {}\n",
    )
    .unwrap();
    let output = run(&["check", "--all-targets"]);
    assert!(
        !output.status.success(),
        "check must detect the broken binary"
    );
    std::fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
    let output = run(&["check", "--all-targets"]);
    assert!(
        output.status.success(),
        "{}\nlock: {}",
        String::from_utf8_lossy(&output.stderr),
        std::fs::read_to_string(root.join("Cargo.lock")).unwrap_or_default()
    );
    for flags in [
        &["test", "--all-targets", "--no-run"][..],
        &["test", "--benches", "--no-run"][..],
    ] {
        let output = run(flags);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
