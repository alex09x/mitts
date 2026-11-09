/*
 * Mitts Build — a path dependency reached through several spellings has one key
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::Path;
use std::process::Command;

fn write(root: &Path, rel: &str, body: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

fn package(name: &str, deps: &str) -> String {
    format!("[package]\nname='{name}'\nversion='0.1.0'\nedition='2021'\n[dependencies]\n{deps}")
}

/// `shared` is a dependency of three members, each spelling the path differently. Which
/// spelling is met first depends on map iteration order, so the unit's source path and its
/// key used to change from run to run and the unit was compiled again on an unchanged tree.
#[test]
fn a_path_dependency_has_one_source_path_and_one_key_whoever_names_it() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("ws");
    write(
        &root,
        "Cargo.toml",
        "[workspace]\nresolver='2'\nmembers=['crates/a','crates/b/inner','crates/c','crates/top']\n",
    );
    write(&root, "crates/shared/Cargo.toml", &package("shared", ""));
    write(&root, "crates/shared/src/lib.rs", "pub fn f() {}\n");
    write(
        &root,
        "crates/a/Cargo.toml",
        &package("a", "shared={path='../shared'}\n"),
    );
    write(&root, "crates/a/src/lib.rs", "pub fn f() { shared::f() }\n");
    write(
        &root,
        "crates/b/inner/Cargo.toml",
        &package("b", "shared={path='../../shared'}\n"),
    );
    write(
        &root,
        "crates/b/inner/src/lib.rs",
        "pub fn f() { shared::f() }\n",
    );
    write(
        &root,
        "crates/c/Cargo.toml",
        &package("c", "shared={path='../a/../shared'}\n"),
    );
    write(&root, "crates/c/src/lib.rs", "pub fn f() { shared::f() }\n");
    write(
        &root,
        "crates/top/Cargo.toml",
        &package(
            "top",
            "a={path='../a'}\nb={path='../b/inner'}\nc={path='../c'}\n",
        ),
    );
    write(
        &root,
        "crates/top/src/lib.rs",
        "pub fn f() { a::f(); b::f(); c::f() }\n",
    );

    let mitts = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_mitts"))
            .current_dir(&root)
            .args(args)
            .arg("--cache-dir")
            .arg(temp.path().join("cache"))
            .output()
            .unwrap()
    };

    let mut keys = std::collections::BTreeSet::new();
    for run in 0..12 {
        let file = temp.path().join(format!("keys-{run}.json"));
        let output = mitts(&["explain", "--keys", file.to_str().unwrap()]);
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.status.success(), "{text}");
        assert!(
            !text.contains("/../"),
            "a unit source keeps a `..` from one dependent's spelling:\n{text}"
        );
        keys.insert(std::fs::read_to_string(file).unwrap());
    }
    assert_eq!(keys.len(), 1, "unit keys differ between identical runs");
}
