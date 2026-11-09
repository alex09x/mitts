/*
 * Mitts Build — --keep-going, example/bench selectors and self dev-dependency regressions
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::Path;
use std::process::{Command, Output};

fn write(root: &Path, rel: &str, body: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

fn mitts(root: &Path, temp: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mitts"))
        .current_dir(root)
        .args(args)
        .arg("--offline")
        .arg("--cache-dir")
        .arg(temp.join("cache"))
        .arg("--target-dir")
        .arg(temp.join("target"))
        .output()
        .unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn package(name: &str, deps: &str) -> String {
    format!("[package]\nname='{name}'\nversion='0.1.0'\nedition='2021'\n[dependencies]\n{deps}")
}

/// Two failing crates in separate chains: `bad` (with `needs-bad` on top of it) and
/// `other-bad` on top of `good-leaf`.
fn keep_going_workspace(root: &Path) {
    write(
        root,
        "Cargo.toml",
        "[workspace]\nresolver='2'\nmembers=['bad','needs-bad','good-leaf','other-bad']\n",
    );
    write(root, "bad/Cargo.toml", &package("bad", ""));
    write(
        root,
        "bad/src/lib.rs",
        "compile_error!(\"KG_FIRST_FAILURE\");\n",
    );
    write(
        root,
        "needs-bad/Cargo.toml",
        &package("needs-bad", "bad={path='../bad'}\n"),
    );
    write(root, "needs-bad/src/lib.rs", "pub fn f() {}\n");
    write(root, "good-leaf/Cargo.toml", &package("good-leaf", ""));
    write(root, "good-leaf/src/lib.rs", "pub fn f() {}\n");
    write(
        root,
        "other-bad/Cargo.toml",
        &package("other-bad", "good-leaf={path='../good-leaf'}\n"),
    );
    write(
        root,
        "other-bad/src/lib.rs",
        "compile_error!(\"KG_SECOND_FAILURE\");\n",
    );
}

#[test]
fn keep_going_reports_every_failure_and_skips_only_dependents() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("ws");
    keep_going_workspace(&root);
    for args in [
        &["check", "--workspace", "--keep-going"][..],
        &["build", "--keep-going"][..],
    ] {
        let command = args[0];
        let output = mitts(&root, temp.path(), args);
        let err = stderr(&output);
        assert!(!output.status.success(), "{command} must fail: {err}");
        assert!(err.contains("KG_FIRST_FAILURE"), "{command}: {err}");
        assert!(err.contains("KG_SECOND_FAILURE"), "{command}: {err}");
        assert!(
            err.contains("skipped 1 unit(s) that depend on a failed unit"),
            "{command}: {err}"
        );
        assert!(
            err.contains("needs-bad") || err.contains("needs_bad"),
            "{command}: {err}"
        );
    }
    let output = mitts(&root, temp.path(), &["check", "--workspace"]);
    let err = stderr(&output);
    assert!(!output.status.success(), "{err}");
    assert!(!err.contains("--keep-going: skipped"), "{err}");
}

#[test]
fn keep_going_succeeds_when_nothing_fails() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("ws");
    keep_going_workspace(&root);
    write(&root, "bad/src/lib.rs", "pub fn f() {}\n");
    write(&root, "other-bad/src/lib.rs", "pub fn f() {}\n");
    let output = mitts(
        &root,
        temp.path(),
        &["check", "--workspace", "--keep-going"],
    );
    assert!(output.status.success(), "{}", stderr(&output));
}

#[test]
fn check_selects_examples_and_benches_by_name_and_as_a_group() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("pkg");
    write(
        &root,
        "Cargo.toml",
        "[package]\nname='sel'\nversion='0.1.0'\nedition='2021'\n[features]\nx=[]\n\
         [[bench]]\nname='b1'\nharness=false\n\
         [[example]]\nname='rf'\nrequired-features=['x']\n",
    );
    write(
        &root,
        "src/lib.rs",
        "#[cfg(feature = \"x\")]\npub fn x() {}\n",
    );
    write(&root, "examples/e1.rs", "fn main() {}\n");
    write(
        &root,
        "examples/e2.rs",
        "compile_error!(\"SEL_EXAMPLE_TWO\");\nfn main() {}\n",
    );
    write(&root, "examples/rf.rs", "fn main() { sel::x(); }\n");
    write(
        &root,
        "benches/b1.rs",
        "compile_error!(\"SEL_BENCH_ONE\");\nfn main() {}\n",
    );
    let run = |args: &[&str]| mitts(&root, temp.path(), args);

    let output = run(&["check", "--example", "e1"]);
    assert!(output.status.success(), "{}", stderr(&output));

    let output = run(&["check", "--example", "e2"]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("SEL_EXAMPLE_TWO"),
        "{}",
        stderr(&output)
    );

    let output = run(&["check", "--example", "e1", "--example", "e2"]);
    assert!(
        stderr(&output).contains("SEL_EXAMPLE_TWO"),
        "{}",
        stderr(&output)
    );

    let output = run(&["check", "--examples"]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("SEL_EXAMPLE_TWO"),
        "{}",
        stderr(&output)
    );
    assert!(
        !stderr(&output).contains("SEL_BENCH_ONE"),
        "{}",
        stderr(&output)
    );

    let output = run(&["check", "--bench", "b1"]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("SEL_BENCH_ONE"),
        "{}",
        stderr(&output)
    );
    assert!(
        !stderr(&output).contains("SEL_EXAMPLE_TWO"),
        "{}",
        stderr(&output)
    );

    let output = run(&["check", "--benches"]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("SEL_BENCH_ONE"),
        "{}",
        stderr(&output)
    );

    let output = run(&["check", "--example", "no-such-example"]);
    assert!(
        !output.status.success(),
        "an unknown example name is an error"
    );

    let output = run(&["check", "--bench", "no-such-bench"]);
    assert!(
        !output.status.success(),
        "an unknown bench name is an error"
    );
    assert!(
        stderr(&output).contains("no bench target named"),
        "{}",
        stderr(&output)
    );

    let output = run(&["clippy", "--example", "e1"]);
    assert!(output.status.success(), "{}", stderr(&output));

    // A broken binary is not part of --examples or --example NAME.
    write(
        &root,
        "src/main.rs",
        "compile_error!(\"SEL_MAIN\");\nfn main() {}\n",
    );
    for args in [
        &["check", "--example", "e1"][..],
        &["build", "--example", "e1"][..],
    ] {
        let output = run(args);
        assert!(output.status.success(), "{args:?}: {}", stderr(&output));
    }
    for command in ["check", "build"] {
        let output = run(&[command, "--examples"]);
        assert!(
            !stderr(&output).contains("SEL_MAIN"),
            "{command}: {}",
            stderr(&output)
        );
        assert!(
            stderr(&output).contains("SEL_EXAMPLE_TWO"),
            "{command}: {}",
            stderr(&output)
        );
    }
    std::fs::remove_file(root.join("src/main.rs")).unwrap();

    let output = run(&["check", "--example", "rf", "--features", "x"]);
    assert!(output.status.success(), "{}", stderr(&output));

    let output = run(&["check", "--example", "rf"]);
    assert!(
        !output.status.success(),
        "an example named without its required features is an error"
    );
}

#[test]
fn a_package_that_dev_depends_on_itself_sees_its_own_library_in_unit_tests() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("pkg");
    write(
        &root,
        "Cargo.toml",
        "[package]\nname='selfdep'\nversion='0.1.0'\nedition='2021'\n[features]\nextra=[]\n\
         [dev-dependencies]\nselfdep={path='.',default-features=false,features=['extra']}\n",
    );
    write(
        &root,
        "src/lib.rs",
        "#[cfg(feature = \"extra\")]\npub fn extra() -> u8 { 7 }\n\
         #[cfg(test)]\nmod tests {\n    #[test]\n    fn uses_own_crate_by_name() {\n        assert_eq!(selfdep::extra(), 7);\n    }\n}\n",
    );
    write(
        &root,
        "tests/it.rs",
        "#[test]\nfn integration() { assert_eq!(selfdep::extra(), 7); }\n",
    );
    let output = mitts(&root, temp.path(), &["check", "--all-targets"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let output = mitts(&root, temp.path(), &["test"]);
    assert!(output.status.success(), "{}", stderr(&output));
}
