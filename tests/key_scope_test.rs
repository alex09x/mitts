/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Cache-key scope: only files the compiler or a build script actually reads may
//! invalidate a unit. Every scenario builds fresh clones against one shared cache,
//! so a reuse can only come from the CAS, never from the fast lock or a warm target.

use mitts::adapter::{RustAdapter, RustBuildOptions};
use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

const MANIFEST: &str = r#"[package]
name = "scope_app"
version = "0.1.0"
edition = "2021"
"#;

fn write_tree(root: &Path, files: &[(&str, &str)]) {
    for (rel, content) in files {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, content).unwrap();
    }
}

/// Builds a fresh clone of `files` against `cache` and returns (all_units_cached, binary stdout).
fn build_clone(cache: &Path, files: &[(&str, &str)]) -> (bool, String) {
    let clone = TempDir::new().unwrap();
    write_tree(clone.path(), files);
    let opt = RustBuildOptions {
        manifest_path: Some(clone.path().join("Cargo.toml")),
        cache_dir: Some(cache.to_path_buf()),
        ..Default::default()
    };
    let (_, res) = RustAdapter::build_native(&opt).expect("build failed");
    assert_eq!(res.exit_code, 0);
    let bin = clone.path().join("target/mitts/debug/scope_app");
    let out = Command::new(&bin).output().expect("run binary");
    (res.cached, String::from_utf8_lossy(&out.stdout).to_string())
}

fn with<'a>(base: &[(&'a str, &'a str)], over: &[(&'a str, &'a str)]) -> Vec<(&'a str, &'a str)> {
    let mut v: Vec<(&str, &str)> = base
        .iter()
        .filter(|(k, _)| !over.iter().any(|(o, _)| o == k))
        .cloned()
        .collect();
    v.extend_from_slice(over);
    v
}

fn count_lines(p: &Path) -> usize {
    fs::read_to_string(p)
        .map(|s| s.lines().count())
        .unwrap_or(0)
}

#[test]
fn ci_readme_and_docs_changes_reuse_cached_units() {
    let cache = TempDir::new().unwrap();
    let base = [
        ("Cargo.toml", MANIFEST),
        (
            "src/main.rs",
            "mod util;\nfn main() { print!(\"V:{}\", util::v()); }\n",
        ),
        ("src/util.rs", "pub fn v() -> u32 { 7 }\n"),
        (".github/workflows/ci.yml", "on: push\n"),
        ("README.md", "readme v1\n"),
        ("LICENSE", "license v1\n"),
        ("docs/guide.md", "guide v1\n"),
    ];
    let (cached_a, out_a) = build_clone(cache.path(), &base);
    assert!(!cached_a);
    assert_eq!(out_a, "V:7");

    let changed = with(
        &base,
        &[
            (".github/workflows/ci.yml", "on: [push, pull_request]\n"),
            ("README.md", "readme v2\n"),
            ("LICENSE", "license v2\n"),
            ("docs/guide.md", "guide v2\n"),
        ],
    );
    let (cached_b, out_b) = build_clone(cache.path(), &changed);
    assert!(
        cached_b,
        "files rustc never reads must not invalidate the unit"
    );
    assert_eq!(out_b, "V:7");
}

#[test]
fn module_file_change_invalidates() {
    let cache = TempDir::new().unwrap();
    let base = [
        ("Cargo.toml", MANIFEST),
        ("src/main.rs", "mod util;\n#[path = \"../gen/extra.rs\"]\nmod extra;\nfn main() { print!(\"V:{}:{}\", util::v(), extra::e()); }\n"),
        ("src/util.rs", "pub fn v() -> u32 { 7 }\n"),
        ("gen/extra.rs", "pub fn e() -> u32 { 1 }\n"),
    ];
    let (_, out_a) = build_clone(cache.path(), &base);
    assert_eq!(out_a, "V:7:1");

    let (cached_b, out_b) = build_clone(
        cache.path(),
        &with(&base, &[("src/util.rs", "pub fn v() -> u32 { 8 }\n")]),
    );
    assert!(!cached_b);
    assert_eq!(out_b, "V:8:1");

    let (cached_c, out_c) = build_clone(
        cache.path(),
        &with(&base, &[("gen/extra.rs", "pub fn e() -> u32 { 2 }\n")]),
    );
    assert!(!cached_c);
    assert_eq!(out_c, "V:7:2");

    let (cached_d, out_d) = build_clone(cache.path(), &base);
    assert!(cached_d);
    assert_eq!(out_d, "V:7:1");
}

#[test]
fn include_str_of_root_and_dot_dir_files_invalidates() {
    let cache = TempDir::new().unwrap();
    let base = [
        ("Cargo.toml", MANIFEST),
        (
            "src/main.rs",
            "const R: &str = include_str!(\"../README.md\");\nconst D: &str = include_str!(\"../.data/x.txt\");\nfn main() { print!(\"{}|{}\", R.trim(), D.trim()); }\n",
        ),
        ("README.md", "r1\n"),
        (".data/x.txt", "d1\n"),
    ];
    let (_, out_a) = build_clone(cache.path(), &base);
    assert_eq!(out_a, "r1|d1");

    let (cached_b, out_b) = build_clone(cache.path(), &with(&base, &[("README.md", "r2\n")]));
    assert!(!cached_b);
    assert_eq!(out_b, "r2|d1");

    let (cached_c, out_c) = build_clone(cache.path(), &with(&base, &[(".data/x.txt", "d2\n")]));
    assert!(!cached_c);
    assert_eq!(out_c, "r1|d2");

    let (cached_d, out_d) = build_clone(cache.path(), &base);
    assert!(cached_d);
    assert_eq!(out_d, "r1|d1");
}

#[test]
fn tests_dir_change_does_not_rerun_build_script_with_directives() {
    let cache = TempDir::new().unwrap();
    let log_dir = TempDir::new().unwrap();
    let log = log_dir.path().join("runs.log");
    let build_rs = format!(
        "use std::io::Write;\nfn main() {{\n    println!(\"cargo:rerun-if-changed=build.rs\");\n    println!(\"cargo:rustc-env=GREETING=hi\");\n    let mut f = std::fs::OpenOptions::new().create(true).append(true).open({:?}).unwrap();\n    writeln!(f, \"run\").unwrap();\n}}\n",
        log
    );
    let base = [
        ("Cargo.toml", MANIFEST),
        ("build.rs", build_rs.as_str()),
        (
            "src/main.rs",
            "fn main() { print!(\"{}\", env!(\"GREETING\")); }\n",
        ),
        ("tests/it.rs", "#[test]\nfn t() { assert_eq!(1, 1); }\n"),
    ];
    let (_, out_a) = build_clone(cache.path(), &base);
    assert_eq!(out_a, "hi");
    assert_eq!(count_lines(&log), 1);

    let changed = with(
        &base,
        &[
            ("tests/it.rs", "#[test]\nfn t() { assert_eq!(2, 2); }\n"),
            ("README.md", "new\n"),
        ],
    );
    let (cached_b, out_b) = build_clone(cache.path(), &changed);
    assert_eq!(out_b, "hi");
    assert_eq!(
        count_lines(&log),
        1,
        "rerun-if-changed=build.rs limits reruns to build.rs, as in Cargo"
    );
    assert!(cached_b);

    let edited = build_rs.replace("GREETING=hi", "GREETING=yo");
    let (cached_c, out_c) =
        build_clone(cache.path(), &with(&base, &[("build.rs", edited.as_str())]));
    assert!(!cached_c);
    assert_eq!(
        out_c, "yo",
        "an edited build script reruns and its output reaches the unit"
    );
    assert_eq!(count_lines(&log), 2);
}

#[test]
fn build_script_without_directives_sees_every_package_file() {
    let cache = TempDir::new().unwrap();
    let build_rs = "fn main() {\n    let v = std::fs::read_to_string(\".cfg/info.txt\").unwrap();\n    println!(\"cargo:rustc-env=INFO={}\", v.trim());\n}\n";
    let base = [
        ("Cargo.toml", MANIFEST),
        ("build.rs", build_rs),
        (
            "src/main.rs",
            "fn main() { print!(\"{}\", env!(\"INFO\")); }\n",
        ),
        (".cfg/info.txt", "i1\n"),
    ];
    let (_, out_a) = build_clone(cache.path(), &base);
    assert_eq!(out_a, "i1");

    let (cached_b, out_b) = build_clone(cache.path(), &with(&base, &[(".cfg/info.txt", "i2\n")]));
    assert!(!cached_b);
    assert_eq!(
        out_b, "i2",
        "without rerun-if directives every package file is a build-script input, as in Cargo"
    );

    let (_, out_c) = build_clone(cache.path(), &base);
    assert_eq!(out_c, "i1");
}

#[test]
fn manifest_metadata_dev_dependencies_and_bench_tables_reuse_cached_units() {
    let cache = TempDir::new().unwrap();
    let base = [
        ("Cargo.toml", MANIFEST),
        (
            "src/main.rs",
            "fn main() { print!(\"{}\", env!(\"CARGO_PKG_VERSION\")); }\n",
        ),
        (
            "helper/Cargo.toml",
            "[package]\nname = \"helper\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        ),
        ("helper/src/lib.rs", "pub fn h() {}\n"),
        ("benches/b.rs", "fn main() {}\n"),
    ];
    let (_, out_a) = build_clone(cache.path(), &base);
    assert_eq!(out_a, "0.1.0");

    let manifest = format!(
        "{MANIFEST}keywords = [\"cache\"]\n\n[package.metadata.docs.rs]\nall-features = true\n\n[badges]\nmaintenance = {{ status = \"actively-developed\" }}\n\n[dev-dependencies]\nhelper = {{ path = \"helper\" }}\n\n[[bench]]\nname = \"b\"\nharness = false\n"
    );
    let (cached_b, out_b) = build_clone(
        cache.path(),
        &with(&base, &[("Cargo.toml", manifest.as_str())]),
    );
    assert!(
        cached_b,
        "registry metadata and test-only tables cannot change the binary"
    );
    assert_eq!(out_b, "0.1.0");
}

#[test]
fn manifest_fields_that_reach_rustc_invalidate() {
    let cache = TempDir::new().unwrap();
    let base = [
        ("Cargo.toml", MANIFEST),
        (
            "src/main.rs",
            "fn main() { print!(\"{}|{}\", env!(\"CARGO_PKG_VERSION\"), option_env!(\"CARGO_PKG_DESCRIPTION\").unwrap_or(\"\")); }\n",
        ),
    ];
    let (_, out_a) = build_clone(cache.path(), &base);
    assert_eq!(out_a, "0.1.0|");

    let bumped = MANIFEST.replace("0.1.0", "0.2.0");
    let (cached_b, out_b) = build_clone(
        cache.path(),
        &with(&base, &[("Cargo.toml", bumped.as_str())]),
    );
    assert!(!cached_b);
    assert_eq!(out_b, "0.2.0|");

    let described = format!("{MANIFEST}description = \"d1\"\n");
    let (cached_c, out_c) = build_clone(
        cache.path(),
        &with(&base, &[("Cargo.toml", described.as_str())]),
    );
    assert!(!cached_c);
    assert_eq!(out_c, "0.1.0|d1");
}
