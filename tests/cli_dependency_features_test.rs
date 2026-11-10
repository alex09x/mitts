/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! `--features dep/feature` on the command line enables a feature of the selected
//! package's dependency (issue #104: `mitts build -p todo --no-default-features
//! --features slint/backend-winit,slint/renderer-software` dropped both features).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

fn write_file(path: impl AsRef<Path>, contents: &str) {
    let path = path.as_ref();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// A slint-shaped workspace: `todo` turns `slint` features on, `slint` forwards them to
/// `selector`, and `selector` switches an optional dependency on (with a weak `winit?/sw`).
fn fixture() -> TempDir {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    write_file(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"todo\", \"slint\", \"selector\", \"winit\"]\nresolver = \"2\"\n",
    );
    write_file(
        root.join("todo/Cargo.toml"),
        "[package]\nname = \"todo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\
         [features]\ndefault = [\"slint/backend-default\"]\n\
         [dependencies]\nslint = { path = \"../slint\", default-features = false, features = [\"std\"] }\n",
    );
    write_file(
        root.join("todo/src/main.rs"),
        "fn main() { slint::report(); }\n",
    );
    write_file(
        root.join("slint/Cargo.toml"),
        "[package]\nname = \"slint\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\
         [features]\ndefault = [\"std\", \"backend-default\"]\nstd = []\n\
         backend-default = [\"selector/backend-default\"]\n\
         backend-winit = [\"selector/backend-winit\"]\n\
         renderer-software = [\"selector/renderer-software\"]\n\
         [dependencies]\nselector = { path = \"../selector\" }\n",
    );
    write_file(
        root.join("slint/src/lib.rs"),
        "pub fn report() {\n\
         println!(\"slint winit={} software={} default={}\", cfg!(feature = \"backend-winit\"), \
         cfg!(feature = \"renderer-software\"), cfg!(feature = \"backend-default\"));\n\
         selector::report();\n}\n",
    );
    write_file(
        root.join("selector/Cargo.toml"),
        "[package]\nname = \"selector\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\
         [features]\nbackend-default = [\"backend-winit\"]\nbackend-winit = [\"dep:winit\"]\n\
         renderer-software = [\"winit?/software\"]\n\
         [dependencies]\nwinit = { path = \"../winit\", optional = true }\n",
    );
    write_file(
        root.join("selector/src/lib.rs"),
        "pub fn report() {\n\
         println!(\"selector winit={}\", cfg!(feature = \"backend-winit\"));\n\
         #[cfg(feature = \"backend-winit\")]\nwinit::report();\n}\n",
    );
    write_file(
        root.join("winit/Cargo.toml"),
        "[package]\nname = \"winit\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\
         [features]\nsoftware = []\n",
    );
    write_file(
        root.join("winit/src/lib.rs"),
        "pub fn report() { println!(\"winit software={}\", cfg!(feature = \"software\")); }\n",
    );
    temp
}

fn mitts(dir: &Path, root: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_mitts"))
        .args(args)
        .arg("--cache-dir")
        .arg(root.join("mitts-cache"))
        .current_dir(dir)
        .output()
        .expect("run Mitts")
}

fn run_todo(root: &Path) -> String {
    let run = Command::new(root.join("target/debug/todo"))
        .output()
        .expect("run todo");
    assert!(run.status.success());
    String::from_utf8(run.stdout).unwrap()
}

fn assert_ok(output: &std::process::Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// With several selected packages, `dep/feature` goes to every selected package that
/// depends on `dep`, also when `dep` is selected itself (review of #105, F1).
#[test]
fn dependency_features_with_several_selected_packages() {
    let temp = fixture();
    let root = temp.path();
    let both = mitts(
        root,
        root,
        &[
            "build",
            "--quiet",
            "-p",
            "todo",
            "-p",
            "winit",
            "--no-default-features",
            "--features",
            "slint/backend-winit",
        ],
    );
    assert_ok(&both);
    assert_eq!(
        run_todo(root),
        "slint winit=true software=false default=false\nselector winit=true\nwinit software=false\n"
    );

    let dependency_selected = mitts(
        root,
        root,
        &[
            "build",
            "--quiet",
            "-p",
            "todo",
            "-p",
            "slint",
            "--no-default-features",
            "--features",
            "slint/backend-winit,slint/renderer-software",
        ],
    );
    assert_ok(&dependency_selected);
    assert_eq!(
        run_todo(root),
        "slint winit=true software=true default=false\nselector winit=true\nwinit software=true\n"
    );

    let unknown = mitts(
        root,
        root,
        &[
            "build",
            "--quiet",
            "-p",
            "todo",
            "-p",
            "winit",
            "--features",
            "nothing/x",
        ],
    );
    assert!(!unknown.status.success());
    assert!(String::from_utf8_lossy(&unknown.stderr).contains("names neither"));
}

/// Inside a member, Mitts builds the enclosing workspace like Cargo: its Cargo.lock (no
/// stray member lockfile) and its members, so `-p <sibling>` works (review of #105, F2).
#[test]
fn member_directory_uses_the_workspace_lock_and_members() {
    let temp = fixture();
    let root = temp.path();
    assert_ok(&mitts(&root.join("todo"), root, &["build", "--quiet"]));
    assert!(root.join("Cargo.lock").is_file());
    assert!(!root.join("todo/Cargo.lock").exists());

    let sibling = mitts(
        &root.join("slint"),
        root,
        &["build", "--quiet", "-p", "todo"],
    );
    assert_ok(&sibling);
    assert!(!root.join("slint/Cargo.lock").exists());
    assert!(!root.join("slint/target").exists());
    assert!(root.join("target/debug/todo").is_file());
}

/// `mitts package` started below a member packages that member into the workspace
/// `target/package` (review of #105, F3).
#[test]
fn package_from_a_member_subdirectory() {
    let temp = fixture();
    let root = temp.path();
    let output = mitts(&root.join("winit/src"), root, &["package", "--no-verify"]);
    assert_ok(&output);
    assert!(root.join("target/package/winit-0.1.0.crate").is_file());
    assert!(!root.join("winit/target").exists());
}

fn build_and_run(root: &Path, args: &[&str]) -> String {
    let cache = root.join("mitts-cache");
    let output = Command::new(env!("CARGO_BIN_EXE_mitts"))
        .args(["build", "--quiet", "--package", "todo"])
        .args(args)
        .arg("--cache-dir")
        .arg(&cache)
        .current_dir(root)
        .output()
        .expect("run Mitts");
    assert!(
        output.status.success(),
        "mitts build {args:?} failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let binary: PathBuf = root.join("target/debug/todo");
    let run = Command::new(binary).output().expect("run todo");
    assert!(run.status.success());
    String::from_utf8(run.stdout).unwrap()
}

/// Like Cargo, a build started below a workspace member finds the nearest manifest by
/// climbing parents and writes into the workspace root's `target`, not the member's.
#[test]
fn member_subdirectory_build_uses_the_workspace_target() {
    let temp = fixture();
    let root = temp.path();
    let output = Command::new(env!("CARGO_BIN_EXE_mitts"))
        .args(["build", "--quiet", "--cache-dir"])
        .arg(root.join("mitts-cache"))
        .current_dir(root.join("todo/src"))
        .output()
        .expect("run Mitts");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(root.join("target/debug/todo").is_file());
    assert!(!root.join("todo/target").exists());
}

/// Started in the member's own directory (its manifest is right there), the build still
/// writes into the workspace root's `target`.
#[test]
fn member_directory_build_uses_the_workspace_target() {
    let temp = fixture();
    let root = temp.path();
    let output = Command::new(env!("CARGO_BIN_EXE_mitts"))
        .args(["build", "--quiet", "--cache-dir"])
        .arg(root.join("mitts-cache"))
        .current_dir(root.join("todo"))
        .output()
        .expect("run Mitts");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(root.join("target/debug/todo").is_file());
    assert!(!root.join("todo/target").exists());
}

#[test]
fn command_line_dependency_features_reach_the_dependency() {
    let temp = fixture();
    let root = temp.path();

    let selected = build_and_run(
        root,
        &[
            "--no-default-features",
            "--features",
            "slint/backend-winit,slint/renderer-software",
        ],
    );
    assert_eq!(
        selected,
        "slint winit=true software=true default=false\n\
         selector winit=true\nwinit software=true\n",
        "dependency features from --features were not applied"
    );

    // Without the dependency features the optional chain stays off: the binary changes,
    // so the previous build cannot be reused for a different feature set.
    let bare = build_and_run(root, &["--no-default-features"]);
    assert_eq!(
        bare,
        "slint winit=false software=false default=false\nselector winit=false\n"
    );

    // A dependency feature adds to the package's own defaults.
    let with_defaults = build_and_run(root, &["--features", "slint/renderer-software"]);
    assert_eq!(
        with_defaults,
        "slint winit=false software=true default=true\n\
         selector winit=true\nwinit software=true\n"
    );
}
