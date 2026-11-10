/*
 * Mitts Build — remaining un-normalized paths (issue 94)
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

fn package(name: &str, deps: &str) -> String {
    format!("[package]\nname='{name}'\nversion='0.1.0'\nedition='2021'\n[dependencies]\n{deps}")
}

fn mitts(root: &Path, cache: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mitts"))
        .current_dir(root)
        .args(args)
        .arg("--cache-dir")
        .arg(cache)
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

/// Unit keys of the workspace at `root`, as `mitts explain --keys` writes them.
fn keys(root: &Path, scratch: &Path) -> String {
    let file = scratch.join("keys.json");
    let out = mitts(
        root,
        &scratch.join("cache"),
        &["explain", "--keys", file.to_str().unwrap()],
    );
    let all = text(&out);
    assert!(out.status.success(), "{all}");
    assert!(!all.contains("/../"), "a unit source keeps a `..`:\n{all}");
    std::fs::read_to_string(file).unwrap()
}

/// The same workspace in two checkouts must have the same unit keys: a key that fell back
/// to an absolute path differs between them.
fn assert_checkout_independent(make: impl Fn(&Path)) {
    let one = tempfile::tempdir().unwrap();
    let two = tempfile::tempdir().unwrap();
    make(one.path());
    make(two.path());
    assert_eq!(
        keys(&one.path().join("ws"), one.path()),
        keys(&two.path().join("ws"), two.path()),
        "unit keys depend on the checkout path"
    );
}

#[test]
fn a_patch_path_through_dot_dot_resolves_inside_a_glob_workspace() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("ws");
    write(
        &root,
        "Cargo.toml",
        "[workspace]\nresolver='2'\nmembers=['crates/*']\n\
         [patch.crates-io]\nfoo = { path = 'crates/app/../foo' }\n",
    );
    write(&root, "crates/foo/Cargo.toml", &package("foo", ""));
    write(&root, "crates/foo/src/lib.rs", "pub fn f() {}\n");
    write(
        &root,
        "crates/app/Cargo.toml",
        &package("app", "foo = '0.1'\n"),
    );
    write(&root, "crates/app/src/lib.rs", "pub fn a() { foo::f() }\n");
    let out = mitts(&root, &temp.path().join("cache"), &["check", "--offline"]);
    assert!(out.status.success(), "{}", text(&out));
}

#[test]
fn an_explicit_target_path_with_dot_dot_keys_relative_to_the_workspace() {
    assert_checkout_independent(|dir| {
        let root = dir.join("ws");
        write(
            &root,
            "Cargo.toml",
            "[workspace]\nresolver='2'\nmembers=['a']\n",
        );
        write(
            &root,
            "a/Cargo.toml",
            &format!("{}[lib]\npath = '../shared/lib.rs'\n", package("a", "")),
        );
        write(&root, "shared/lib.rs", "pub fn f() {}\n");
    });
}

#[test]
fn two_glob_members_with_one_package_name_are_refused() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("ws");
    write(
        &root,
        "Cargo.toml",
        "[workspace]\nresolver='2'\nmembers=['crates/*']\n",
    );
    for dir in ["crates/one", "crates/two"] {
        write(&root, &format!("{dir}/Cargo.toml"), &package("same", ""));
        write(&root, &format!("{dir}/src/lib.rs"), "pub fn f() {}\n");
    }
    let out = mitts(&root, &temp.path().join("cache"), &["check", "--offline"]);
    let all = text(&out);
    assert!(!out.status.success(), "{all}");
    assert!(all.contains("two packages named `same`"), "{all}");
}

#[cfg(unix)]
#[test]
fn a_path_dependency_reached_through_a_symlink_keys_by_its_workspace_path() {
    assert_checkout_independent(|dir| {
        let root = dir.join("ws");
        write(
            &root,
            "Cargo.toml",
            "[workspace]\nresolver='2'\nmembers=['app']\n",
        );
        write(&root, "vendor/dep/Cargo.toml", &package("dep", ""));
        write(&root, "vendor/dep/src/lib.rs", "pub fn d() {}\n");
        // `link` sits outside the workspace and points back into it.
        std::os::unix::fs::symlink("ws/vendor/dep", dir.join("link")).unwrap();
        write(
            &root,
            "app/Cargo.toml",
            &package("app", "dep = { path = '../../link' }\n"),
        );
        write(&root, "app/src/lib.rs", "pub fn a() { dep::d() }\n");
    });
}
