/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Issue 143 on riscv64: skia-bindings' build script runs `skia/bin/fetch-gn`, which has no
//! riscv64 download; the reporter replaced it by hand inside Cargo's unpacked copy
//! (`$CARGO_HOME/registry/src/.../skia-bindings-0.157.0`) and Cargo built. Mitts built its own
//! pristine copy from the source store and failed. Cargo builds the unpacked copy as it finds
//! it, so Mitts does too; an edited copy must never be keyed or stored as the published one.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Duration;
use tempfile::TempDir;

const INDEX_DIR: &str = "index.crates.io-1949cf8c6b5b557f";

fn write_file(path: impl AsRef<Path>, contents: &str) {
    let path = path.as_ref();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// Unpacks `helper` 0.1.0 the way Cargo does: every file first, `.cargo-ok` last. Its build
/// script fails while `tool/fetch` says `fail`, like `fetch-gn` on riscv64.
fn unpack_helper(cargo_home: &Path) -> PathBuf {
    let dir = cargo_home
        .join("registry/src")
        .join(INDEX_DIR)
        .join("helper-0.1.0");
    if dir.exists() {
        fs::remove_dir_all(&dir).unwrap();
    }
    write_file(
        dir.join("Cargo.toml"),
        "[package]\nname = \"helper\"\nversion = \"0.1.0\"\nedition = \"2021\"\nbuild = \"build.rs\"\n",
    );
    write_file(
        dir.join("build.rs"),
        "fn main() {\n\
         let tool = std::fs::read_to_string(\"tool/fetch\").unwrap();\n\
         assert!(tool.trim() != \"fail\", \"KeyError: 'riscv64'\");\n\
         println!(\"cargo:rustc-env=HELPER_TOOL={}\", tool.trim());\n}\n",
    );
    write_file(
        dir.join("src/lib.rs"),
        // Cargo caps lints for a registry package, edited or not.
        "#![deny(warnings)]\nfn unused() {}\npub fn tool() -> &'static str { env!(\"HELPER_TOOL\") }\n",
    );
    write_file(dir.join("tool/fetch"), "fail\n");
    std::thread::sleep(Duration::from_millis(30));
    fs::write(dir.join(".cargo-ok"), "{\"v\":1}").unwrap();
    std::thread::sleep(Duration::from_millis(30));
    dir
}

fn project(root: &Path) {
    write_file(
        root.join("app/Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\
         [dependencies]\nhelper = \"0.1.0\"\n",
    );
    write_file(
        root.join("app/Cargo.lock"),
        "version = 3\n\n[[package]]\nname = \"app\"\nversion = \"0.1.0\"\ndependencies = [\"helper\"]\n\n\
         [[package]]\nname = \"helper\"\nversion = \"0.1.0\"\n\
         source = \"registry+https://github.com/rust-lang/crates.io-index\"\n\
         checksum = \"0000000000000000000000000000000000000000000000000000000000000000\"\n",
    );
    write_file(
        root.join("app/src/main.rs"),
        "fn main() { println!(\"tool={}\", helper::tool()); }\n",
    );
}

fn mitts(root: &Path, args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mitts"));
    cmd.args(args)
        .arg("--cache-dir")
        .arg(root.join("mitts-cache"))
        .current_dir(root.join("app"))
        .env("CARGO_HOME", root.join("cargo-home"))
        .env_remove("MITTS_CACHE_DIR");
    for name in [
        "CARGO_TARGET_DIR",
        "CARGO_BUILD_TARGET_DIR",
        "CARGO_BUILD_TARGET",
        "CARGO_ENCODED_RUSTFLAGS",
        "RUSTFLAGS",
        "CARGO_BUILD_RUSTFLAGS",
        "RUSTC_WRAPPER",
        "RUSTC_WORKSPACE_WRAPPER",
        "CARGO_BUILD_RUSTC_WRAPPER",
    ] {
        cmd.env_remove(name);
    }
    cmd.output().expect("run Mitts")
}

fn build(root: &Path) -> Output {
    mitts(root, &["build", "--offline"])
}

fn run_app(root: &Path) -> String {
    let out = Command::new(root.join("app/target/debug/app"))
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
}

#[test]
fn a_hand_edited_cargo_registry_copy_is_built_like_cargo_and_never_cached_as_published() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    let cargo_home = root.join("cargo-home");
    project(root);
    let helper = unpack_helper(&cargo_home);

    // The published package fails, as skia-bindings does on riscv64.
    let out = build(root);
    assert!(!out.status.success(), "the pristine helper must fail");
    assert!(stderr(&out).contains("riscv64"), "{}", stderr(&out));

    // The reporter's workaround, inside Cargo's copy.
    fs::write(helper.join("tool/fetch"), "stub\n").unwrap();
    let out = build(root);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(run_app(root), "tool=stub");
    let stored = root.join("mitts-cache/sources/helper-0.1.0/tool/fetch");
    assert!(
        fs::read_to_string(&stored).map_or(true, |s| s == "fail\n"),
        "an edited copy never reaches the source store"
    );

    // Unpacked again (`cargo clean` of the registry, a new machine): the published package
    // must fail again, not come back from the cache entry the edited copy produced.
    unpack_helper(&cargo_home);
    let out = build(root);
    assert!(
        !out.status.success(),
        "a cache entry of the edited copy was served for the published package"
    );
}

/// The reporter had built with Mitts before, so the source store already held the published
/// helper; the edit in Cargo's copy still wins, as it does for Cargo.
#[test]
fn an_edited_cargo_copy_wins_over_the_published_copy_in_the_source_store() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    project(root);
    let helper = unpack_helper(&root.join("cargo-home"));
    let stored = root.join("mitts-cache/sources/helper-0.1.0");
    for file in ["Cargo.toml", "build.rs", "src/lib.rs", "tool/fetch"] {
        let contents = fs::read_to_string(helper.join(file)).unwrap();
        write_file(stored.join(file), &contents);
    }
    let marker = stored.join(mitts::core::source_store::MITTS_OK_MARKER);
    fs::write(marker, "ok\n").unwrap();
    assert!(!build(root).status.success(), "the stored helper fails");

    fs::write(helper.join("tool/fetch"), "stub\n").unwrap();
    let out = build(root);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(run_app(root), "tool=stub");
}

#[test]
fn only_an_untouched_cargo_copy_counts_as_the_published_package() {
    use mitts::core::cargo_registry_copy::{is_edited, is_pristine, reset};
    let temp = TempDir::new().unwrap();
    let cargo_home = temp.path().join("cargo-home");

    let dir = unpack_helper(&cargo_home);
    reset();
    assert!(is_pristine(&dir) && !is_edited(&dir));

    fs::write(dir.join("tool/fetch"), "stub\n").unwrap();
    reset();
    assert!(is_edited(&dir), "an edited file");

    let dir = unpack_helper(&cargo_home);
    #[cfg(unix)]
    std::os::unix::fs::symlink("/usr/bin/true", dir.join("tool/gn")).unwrap();
    #[cfg(not(unix))]
    fs::write(dir.join("tool/gn"), "").unwrap();
    reset();
    assert!(is_edited(&dir), "an added file");

    let dir = unpack_helper(&cargo_home);
    fs::remove_file(dir.join("build.rs")).unwrap();
    reset();
    assert!(is_edited(&dir), "a removed top-level file");

    let half = cargo_home
        .join("registry/src")
        .join(INDEX_DIR)
        .join("half-0.1.0");
    write_file(half.join("Cargo.toml"), "[package]\nname = \"half\"\n");
    reset();
    assert!(
        !is_pristine(&half) && !is_edited(&half),
        "without `.cargo-ok` it is not a Cargo copy at all"
    );
}
