/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A git dependency is an immutable external source, like a registry crate, and a coverage
//! report leaves it out. A repository with one crate at its root is checked out to
//! `<cache>/git/checkouts/<repo>/<rev>/`, which holds the `.mitts-ok` marker and is the package
//! directory itself; the marker search used to skip that directory, so such a dependency was
//! treated as workspace code and appeared as `external/<crate>-<version>/src/...`.

use mitts::adapter::rust::inputs::is_immutable_package_dir;
use mitts::cli::coverage_tools::resolve_llvm_tool;
use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

fn mark(dir: &Path) {
    fs::create_dir_all(dir).unwrap();
    fs::write(dir.join(".mitts-ok"), "").unwrap();
    fs::write(dir.join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
}

#[test]
fn a_crate_at_the_root_of_a_git_checkout_is_immutable() {
    let cache = TempDir::new().unwrap();
    let revision = cache.path().join("git/checkouts/repo-1a2b3c/0123abc");
    mark(&revision);
    assert!(is_immutable_package_dir(&revision, false));
}

#[test]
fn a_crate_in_a_subdirectory_of_a_git_checkout_is_immutable() {
    let cache = TempDir::new().unwrap();
    let revision = cache.path().join("git/checkouts/repo-1a2b3c/0123abc");
    mark(&revision);
    let member = revision.join("crates/member");
    fs::create_dir_all(&member).unwrap();
    assert!(is_immutable_package_dir(&member, false));
}

#[test]
fn only_a_marked_git_checkout_counts() {
    let cache = TempDir::new().unwrap();
    let unmarked = cache.path().join("git/checkouts/repo-1a2b3c/0123abc");
    fs::create_dir_all(&unmarked).unwrap();
    assert!(!is_immutable_package_dir(&unmarked, false), "no marker");

    let elsewhere = cache.path().join("projects/repo-1a2b3c/0123abc");
    mark(&elsewhere);
    assert!(
        !is_immutable_package_dir(&elsewhere, false),
        "a marker outside git/checkouts does not make a package immutable"
    );

    let wrong_depth = cache.path().join("git/checkouts/0123abc");
    mark(&wrong_depth);
    assert!(!is_immutable_package_dir(&wrong_depth, false));
}

#[test]
fn a_workspace_package_is_never_immutable() {
    let cache = TempDir::new().unwrap();
    let revision = cache.path().join("git/checkouts/repo-1a2b3c/0123abc");
    mark(&revision);
    assert!(!is_immutable_package_dir(&revision, true));
}

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(dir)
        .args(["-c", "user.email=t@example.com", "-c", "user.name=t"])
        .args(args)
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?}");
}

#[test]
fn coverage_leaves_a_git_dependency_out_of_the_report() {
    let tools =
        resolve_llvm_tool("llvm-profdata").is_some() && resolve_llvm_tool("llvm-cov").is_some();
    let has_git = Command::new("git").arg("--version").output().is_ok();
    if !tools || !has_git {
        eprintln!("skipped: llvm-tools or git are not installed");
        return;
    }
    let dir = TempDir::new().unwrap();
    let dep = dir.path().join("dep");
    let app = dir.path().join("app");
    for path in [dep.join("src"), app.join("src")] {
        fs::create_dir_all(path).unwrap();
    }
    fs::write(
        dep.join("Cargo.toml"),
        "[package]\nname = \"gitdep\"\nversion = \"0.2.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(
        dep.join("src/lib.rs"),
        "pub fn helper() -> u32 { 1 }\npub fn dead_one() -> u32 { 8 }\npub fn dead_two() -> u32 { 9 }\n",
    )
    .unwrap();
    git(&dep, &["init", "-q", "."]);
    git(&dep, &["add", "-A"]);
    git(&dep, &["commit", "-q", "-m", "init"]);

    fs::write(
        app.join("Cargo.toml"),
        format!(
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
             [dependencies]\ngitdep = {{ git = \"file://{}\" }}\n",
            dep.display()
        ),
    )
    .unwrap();
    fs::write(
        app.join("src/lib.rs"),
        "pub fn covered() -> u32 { gitdep::helper() + 1 }\n\n\
         #[cfg(test)]\nmod tests {\n    #[test]\n    fn covers() { assert_eq!(super::covered(), 2); }\n}\n",
    )
    .unwrap();

    let lcov = dir.path().join("out/report.lcov");
    let output = Command::new(env!("CARGO_BIN_EXE_mitts"))
        .current_dir(&app)
        .args(["llvm-cov", "--lcov", "--output-path"])
        .arg(&lcov)
        .arg("--cache-dir")
        .arg(dir.path().join("cache"))
        .env("CARGO_HOME", dir.path().join("cargo-home"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let report = fs::read_to_string(&lcov).unwrap();
    let files: Vec<&str> = report
        .lines()
        .filter_map(|line| line.strip_prefix("SF:"))
        .collect();
    assert_eq!(
        files,
        ["src/lib.rs"],
        "only the measured crate is reported; the git dependency is left out"
    );
}
