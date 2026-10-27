/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A library's dev-dependencies often pull in an older published version of the library
//! itself (ahash's tests depend on hashbrown, which depends on ahash). That older copy
//! shares the member's name, and the planner used to treat it as the primary package:
//! root features (`default`) were requested for it although its dependants had turned
//! them off, so rustc got `--cfg feature="…"` for an optional dependency that the lockfile
//! never activated and failed with "cannot find crate". The role belongs to the workspace
//! member only.

use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

/// `foo` 0.2.0 (member, default feature `extra` → optional `helper`) with a dev-dependency
/// on `foo` 0.1.0 (path, `default-features = false`), the same shape as the member.
fn write_project(dir: &Path) {
    for sub in ["src", "tests", "old/src", "helper/src"] {
        fs::create_dir_all(dir.join(sub)).unwrap();
    }
    fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname = \"foo\"\nversion = \"0.2.0\"\nedition = \"2021\"\n\n\
         [dependencies]\nhelper = { path = \"helper\", optional = true }\n\n\
         [dev-dependencies]\nfoo_old = { package = \"foo\", path = \"old\", default-features = false }\n\n\
         [features]\ndefault = [\"extra\"]\nextra = [\"dep:helper\"]\n\n\
         [workspace]\nexclude = [\"old\", \"helper\"]\n",
    )
    .unwrap();
    fs::write(
        dir.join("old/Cargo.toml"),
        "[package]\nname = \"foo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
         [dependencies]\nhelper = { path = \"../helper\", optional = true }\n\n\
         [features]\ndefault = [\"extra\"]\nextra = [\"dep:helper\"]\n",
    )
    .unwrap();
    fs::write(
        dir.join("helper/Cargo.toml"),
        "[package]\nname = \"helper\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(dir.join("helper/src/lib.rs"), "pub fn seven() -> u64 { 7 }\n").unwrap();
    let lib = "#[cfg(feature = \"extra\")]\npub fn val() -> u64 { helper::seven() }\n\
               #[cfg(not(feature = \"extra\"))]\npub fn val() -> u64 { 1 }\n";
    fs::write(dir.join("src/lib.rs"), lib).unwrap();
    fs::write(dir.join("old/src/lib.rs"), lib).unwrap();
    fs::write(
        dir.join("tests/versions.rs"),
        "#[test]\nfn member_has_default_features_and_the_old_copy_does_not() {\n\
         \x20   assert_eq!(foo::val(), 7);\n    assert_eq!(foo_old::val(), 1);\n}\n",
    )
    .unwrap();
}

#[test]
fn an_older_copy_of_the_member_keeps_its_own_features() {
    let cache = TempDir::new().unwrap();
    let dir = TempDir::new().unwrap();
    write_project(dir.path());
    // The lockfile Cargo writes names the duplicate as `foo 0.1.0`, as in a checked-in lock.
    let lock = Command::new("cargo")
        .args(["generate-lockfile", "--offline"])
        .current_dir(dir.path())
        .output()
        .expect("run cargo generate-lockfile");
    assert!(lock.status.success(), "{}", String::from_utf8_lossy(&lock.stderr));

    let out = Command::new(env!("CARGO_BIN_EXE_mitts"))
        .args(["test", "--manifest-path"])
        .arg(dir.path().join("Cargo.toml"))
        .arg("--cache-dir")
        .arg(cache.path())
        .env("CI", "true")
        .env_remove("MITTS_CACHE_DIR")
        .output()
        .expect("run mitts");
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.status.success(),
        "the old copy must compile without the member's features:\n{text}"
    );
    assert!(
        text.contains("member_has_default_features_and_the_old_copy_does_not ... ok"),
        "the test against both versions ran and passed:\n{text}"
    );
}
