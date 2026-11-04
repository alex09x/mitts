/*
 * Mitts Build — Cargo-compatible output and selective cleanup regressions.
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::cli::handlers::handle_clean;
use mitts::cli::CleanArgs;
use mitts::core::hash::Sha256;
use mitts::core::TargetArtifactIndex;
use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

fn fixture(root: &Path) {
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"layout-app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(root.join("src/main.rs"), "fn main() { println!(\"{}\", option_env!(\"MITTS_LAYOUT_TEST_MODE\").unwrap_or(\"mitts\")); }\n").unwrap();
}

fn build_mitts(root: &Path, release: bool) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mitts"));
    cmd.current_dir(root)
        .args(["build", "--native", "--cache-dir"])
        .arg(root.join("cache"))
        .env("MITTS_FAST_LOCK", "1")
        .env_remove("CARGO_TARGET_DIR")
        .env_remove("MITTS_LAYOUT_TEST_MODE")
        .env_remove("CARGO_MAKEFLAGS")
        .env_remove("MAKEFLAGS");
    if release {
        cmd.arg("--release");
    }
    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn run(path: &Path) -> String {
    let output = Command::new(path).output().unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

#[test]
fn default_outputs_recover_after_another_mitts_build() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    fixture(root);
    build_mitts(root, false);
    let binary = root.join("target/debug/layout-app");
    assert_eq!(run(&binary), "mitts");
    assert!(!root.join("target/mitts/debug/layout-app").exists());
    assert!(!root.join(".mitts-clean.lock").exists());

    let output = Command::new(env!("CARGO_BIN_EXE_mitts"))
        .current_dir(root)
        .args(["build", "--offline", "--target-dir"])
        .arg(root.join("target"))
        .arg("--cache-dir")
        .arg(root.join("other-cache"))
        .env("MITTS_LAYOUT_TEST_MODE", "cargo")
        .env_remove("CARGO_MAKEFLAGS")
        .env_remove("MAKEFLAGS")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(run(&binary), "cargo");
    build_mitts(root, false);
    assert_eq!(run(&binary), "mitts");
    build_mitts(root, true);
    assert_eq!(run(&root.join("target/release/layout-app")), "mitts");
}

fn record(index: &mut TargetArtifactIndex, target: &Path, relative: &str, contents: &[u8]) {
    let path = target.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, contents).unwrap();
    index.record(
        relative.to_owned(),
        Sha256::digest_file(&path).unwrap(),
        &path,
    );
}

#[test]
fn clean_preserves_cargo_files_and_replaced_outputs_in_a_shared_root() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    fixture(root);
    let target = root.join("target");
    fs::create_dir_all(&target).unwrap();
    fs::write(target.join(".mitts-target"), "mitts-target-v1\n").unwrap();
    let mut debug = TargetArtifactIndex::default();
    record(&mut debug, &target, "debug/owned", b"mitts output");
    record(&mut debug, &target, "debug/replaced", b"old output");
    record(
        &mut debug,
        &target,
        "debug/deps/libowned-abcd.rlib",
        b"library",
    );
    fs::hard_link(
        target.join("debug/deps/libowned-abcd.rlib"),
        target.join("debug/deps/libowned.rlib"),
    )
    .unwrap();
    let alias = target.join("debug/deps/libowned.rlib");
    debug.record(
        "debug/deps/libowned.rlib".into(),
        Sha256::digest_file(&alias).unwrap(),
        &alias,
    );
    debug.save(&target, "debug").unwrap();
    fs::remove_file(target.join("debug/replaced")).unwrap();
    fs::write(target.join("debug/replaced"), b"Cargo replacement").unwrap();
    fs::write(target.join("debug/cargo-only"), b"Cargo output").unwrap();
    let mut release = TargetArtifactIndex::default();
    record(&mut release, &target, "release/owned", b"release output");
    release.save(&target, "release").unwrap();
    let mut args = CleanArgs {
        manifest_path: Some(root.join("Cargo.toml")),
        target_dir: Some(target.clone()),
        release: true,
        ..Default::default()
    };
    assert_eq!(handle_clean(&args), 0);
    assert!(!target.join("release/owned").exists());
    assert!(target.join("debug/owned").exists());
    args.release = false;
    assert_eq!(handle_clean(&args), 0);
    assert!(!target.join("debug/owned").exists());
    assert!(!target.join("debug/deps/libowned.rlib").exists());
    assert_eq!(
        fs::read(target.join("debug/replaced")).unwrap(),
        b"Cargo replacement"
    );
    assert_eq!(
        fs::read(target.join("debug/cargo-only")).unwrap(),
        b"Cargo output"
    );
    assert_eq!(handle_clean(&args), 0);
}

#[test]
fn clean_rejects_parent_traversal_and_symlinked_parents() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    fixture(root);
    let target = root.join("target");
    fs::create_dir_all(target.join("debug")).unwrap();
    fs::write(target.join(".mitts-target"), "mitts-target-v1\n").unwrap();
    let outside = root.join("outside");
    fs::write(&outside, b"preserve").unwrap();
    let mut index = TargetArtifactIndex::default();
    let hash = Sha256::digest_file(&outside).unwrap();
    index.record("../outside".into(), hash.clone(), &outside);
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(root, target.join("debug/escape")).unwrap();
        index.record("debug/escape/outside".into(), hash, &outside);
    }
    index.save(&target, "debug").unwrap();
    assert_eq!(
        handle_clean(&CleanArgs {
            manifest_path: Some(root.join("Cargo.toml")),
            target_dir: Some(target),
            ..Default::default()
        }),
        0
    );
    assert_eq!(fs::read(outside).unwrap(), b"preserve");
}

#[test]
fn clean_skips_a_locked_profile_and_preserves_foreign_replacement() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    fixture(root);
    build_mitts(root, false);
    let binary = root.join("target/debug/layout-app");
    let guard =
        mitts::core::lock::FileLock::acquire_exclusive(&root.join("target/debug/.cargo-lock"))
            .unwrap();
    let mut clean = Command::new(env!("CARGO_BIN_EXE_mitts"));
    clean
        .current_dir(root)
        .args(["clean", "--target-dir"])
        .arg(root.join("target"))
        .env_remove("CARGO_TARGET_DIR");
    let attempt = clean.output().unwrap();
    let skipped = !attempt.status.success()
        && String::from_utf8_lossy(&attempt.stderr).contains("Cargo build is active");
    let preserved = binary.exists();
    assert!(
        skipped && preserved,
        "clean must skip an active Cargo build without deleting its output"
    );
    fs::remove_file(&binary).unwrap();
    fs::write(&binary, b"foreign replacement").unwrap();
    drop(guard);
    assert!(clean.output().unwrap().status.success());
    assert_eq!(fs::read(&binary).unwrap(), b"foreign replacement");
}
