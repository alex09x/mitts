/*
 * Mitts Build — target selection follow-ups (issue 90)
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
        .env_remove("MITTS_CACHE_DIR")
        .output()
        .unwrap()
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

const PKG: &str = "[package]\nname='pkg'\nversion='0.1.0'\nedition='2021'\n";

#[test]
fn a_self_dev_dependency_under_another_name_is_importable_in_unit_tests() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("ws");
    write(
        &root,
        "Cargo.toml",
        &format!("{PKG}[dev-dependencies]\nme = {{ path = '.', package = 'pkg' }}\n"),
    );
    write(
        &root,
        "src/lib.rs",
        "pub fn f() -> u32 { 4 }\n#[cfg(test)]\nmod t { #[test] fn via_alias() { assert_eq!(me::f(), 4); } }\n",
    );
    let out = mitts(&root, temp.path(), &["test"]);
    assert!(out.status.success(), "{}", text(&out));
}

#[test]
fn an_empty_group_selection_checks_nothing_and_warns() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("ws");
    write(&root, "Cargo.toml", PKG);
    // Anything checked would fail: the selection must not fall back to the library.
    write(
        &root,
        "src/lib.rs",
        "compile_error!(\"LIB_WAS_CHECKED\");\n",
    );
    for flag in ["--benches", "--examples"] {
        let out = mitts(&root, temp.path(), &["check", flag]);
        let all = text(&out);
        assert!(out.status.success(), "check {flag}: {all}");
        assert!(!all.contains("LIB_WAS_CHECKED"), "check {flag}: {all}");
        assert!(all.contains("no targets matched"), "check {flag}: {all}");
    }
}

#[test]
fn a_named_bench_left_out_by_required_features_names_them() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("ws");
    write(
        &root,
        "Cargo.toml",
        &format!(
            "{PKG}[features]\nfast = []\n[[bench]]\nname = 'speed'\nrequired-features = ['fast']\n"
        ),
    );
    write(&root, "src/lib.rs", "pub fn f() {}\n");
    write(&root, "benches/speed.rs", "#[test]\nfn b() {}\n");
    let out = mitts(&root, temp.path(), &["check", "--bench", "speed"]);
    let all = text(&out);
    assert!(!out.status.success(), "{all}");
    assert!(all.contains("requires the features: `fast`"), "{all}");
    let ok = mitts(
        &root,
        temp.path(),
        &["check", "--bench", "speed", "--features", "fast"],
    );
    assert!(ok.status.success(), "{}", text(&ok));
}

#[test]
fn check_lib_and_benches_does_not_check_unit_test_code() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("ws");
    write(&root, "Cargo.toml", PKG);
    write(
        &root,
        "src/lib.rs",
        "pub fn f() {}\n#[cfg(test)]\ncompile_error!(\"UNIT_TESTS_CHECKED\");\n",
    );
    write(&root, "benches/speed.rs", "#[test]\nfn b() { pkg::f() }\n");
    let out = mitts(&root, temp.path(), &["check", "--lib", "--benches"]);
    assert!(out.status.success(), "{}", text(&out));
}

#[test]
fn check_all_targets_skips_unit_tests_of_a_test_false_library() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("ws");
    write(&root, "Cargo.toml", &format!("{PKG}[lib]\ntest = false\n"));
    write(
        &root,
        "src/lib.rs",
        "pub fn f() {}\n#[cfg(test)]\ncompile_error!(\"UNIT_TESTS_CHECKED\");\n",
    );
    let out = mitts(&root, temp.path(), &["check", "--all-targets"]);
    assert!(out.status.success(), "{}", text(&out));
}

#[test]
fn test_keep_going_reports_every_broken_test_target() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("ws");
    write(&root, "Cargo.toml", PKG);
    write(&root, "src/lib.rs", "pub fn f() {}\n");
    write(&root, "tests/a.rs", "compile_error!(\"FIRST_BROKEN\");\n");
    write(&root, "tests/b.rs", "compile_error!(\"SECOND_BROKEN\");\n");
    let out = mitts(&root, temp.path(), &["test", "--keep-going"]);
    let all = text(&out);
    assert!(!out.status.success(), "{all}");
    assert!(
        all.contains("FIRST_BROKEN") && all.contains("SECOND_BROKEN"),
        "both broken targets must be compiled and reported: {all}"
    );
}

#[test]
fn build_selects_benches_by_name_and_as_a_group() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("ws");
    write(&root, "Cargo.toml", PKG);
    write(&root, "src/lib.rs", "pub fn f() {}\n");
    write(
        &root,
        "benches/speed.rs",
        "compile_error!(\"BENCH_BUILT\");\n",
    );
    let plain = mitts(&root, temp.path(), &["build"]);
    assert!(plain.status.success(), "{}", text(&plain));
    for args in [
        &["build", "--bench", "speed"][..],
        &["build", "--benches"][..],
    ] {
        let out = mitts(&root, temp.path(), args);
        let all = text(&out);
        assert!(
            !out.status.success() && all.contains("BENCH_BUILT"),
            "{args:?}: {all}"
        );
    }
    let missing = mitts(&root, temp.path(), &["build", "--bench", "nosuch"]);
    assert!(text(&missing).contains("no bench target named `nosuch`"));
}

#[test]
fn check_and_clippy_reject_unknown_test_and_bin_names() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("ws");
    write(&root, "Cargo.toml", PKG);
    write(&root, "src/lib.rs", "pub fn f() {}\n");
    for cmd in ["check", "clippy"] {
        let out = mitts(&root, temp.path(), &[cmd, "--test", "nosuch"]);
        let all = text(&out);
        assert!(!out.status.success(), "{cmd}: {all}");
        assert!(
            all.contains("no test target named `nosuch`"),
            "{cmd}: {all}"
        );
        let out = mitts(&root, temp.path(), &[cmd, "--bin", "nosuch"]);
        let all = text(&out);
        assert!(!out.status.success(), "{cmd}: {all}");
        assert!(all.contains("no bin target named `nosuch`"), "{cmd}: {all}");
    }
}

#[test]
fn test_benches_on_a_library_without_benches_is_a_no_op() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("ws");
    write(&root, "Cargo.toml", PKG);
    write(
        &root,
        "src/lib.rs",
        "/// ```\n/// assert!(true);\n/// ```\npub fn f() {}\n",
    );
    for args in [&["test", "--benches"][..], &["test", "--all-targets"][..]] {
        let out = mitts(&root, temp.path(), args);
        let all = text(&out);
        assert!(out.status.success(), "{args:?}: {all}");
        assert!(
            !all.contains("Doc-tests") && !all.contains("doctest"),
            "{args:?} ran doctests: {all}"
        );
    }
}

#[test]
fn a_named_target_of_the_other_group_survives_an_empty_group_filter() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("ws");
    write(&root, "Cargo.toml", PKG);
    write(&root, "src/lib.rs", "pub fn f() {}\n");
    write(&root, "benches/speed.rs", "#[test]\nfn b() {}\n");
    write(&root, "examples/demo.rs", "fn main() {}\n");
    // No examples group match is needed for `--bench speed` to stay selected, and the reverse.
    let only_bench = tempfile::tempdir().unwrap();
    let r2 = only_bench.path().join("ws");
    write(&r2, "Cargo.toml", PKG);
    write(&r2, "src/lib.rs", "pub fn f() {}\n");
    write(&r2, "benches/speed.rs", "#[test]\nfn b() {}\n");
    let out = mitts(
        &r2,
        only_bench.path(),
        &["check", "--examples", "--bench", "speed"],
    );
    assert!(out.status.success(), "{}", text(&out));
    let only_example = tempfile::tempdir().unwrap();
    let r3 = only_example.path().join("ws");
    write(&r3, "Cargo.toml", PKG);
    write(&r3, "src/lib.rs", "pub fn f() {}\n");
    write(&r3, "examples/demo.rs", "fn main() {}\n");
    let out = mitts(
        &r3,
        only_example.path(),
        &["check", "--benches", "--example", "demo"],
    );
    assert!(out.status.success(), "{}", text(&out));
    let both = mitts(
        &root,
        temp.path(),
        &["check", "--examples", "--bench", "speed"],
    );
    assert!(both.status.success(), "{}", text(&both));
}

#[test]
fn a_renamed_self_dev_dependency_is_importable_in_integration_tests_and_examples() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("ws");
    write(
        &root,
        "Cargo.toml",
        &format!("{PKG}[dev-dependencies]\nme = {{ path = '.', package = 'pkg' }}\n"),
    );
    write(&root, "src/lib.rs", "pub fn f() -> u32 { 4 }\n");
    write(
        &root,
        "tests/it.rs",
        "#[test]\nfn via_alias() { assert_eq!(me::f(), 4); }\n",
    );
    write(&root, "examples/ex.rs", "fn main() { let _ = me::f(); }\n");
    let out = mitts(&root, temp.path(), &["test"]);
    assert!(out.status.success(), "{}", text(&out));
    let out = mitts(&root, temp.path(), &["check", "--examples"]);
    assert!(out.status.success(), "{}", text(&out));
}

#[test]
fn bench_and_test_names_are_not_interchangeable() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("ws");
    write(&root, "Cargo.toml", PKG);
    write(&root, "src/lib.rs", "pub fn f() {}\n");
    write(&root, "tests/api.rs", "#[test]\nfn t() {}\n");
    write(&root, "benches/speed.rs", "#[test]\nfn b() {}\n");
    let out = mitts(&root, temp.path(), &["check", "--tests", "--bench", "api"]);
    let all = text(&out);
    assert!(!out.status.success(), "{all}");
    assert!(all.contains("no bench target named `api`"), "{all}");
    let out = mitts(
        &root,
        temp.path(),
        &["check", "--benches", "--test", "speed"],
    );
    let all = text(&out);
    assert!(!out.status.success(), "{all}");
    assert!(all.contains("no test target named `speed`"), "{all}");
}
