/*
 * Mitts Build — profile `incremental` reaches rustc for workspace units (issue 97)
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn write(root: &Path, rel: &str, body: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

fn build(root: &Path, temp: &Path, extra: &[&str], env: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mitts"));
    cmd.current_dir(root)
        .arg("build")
        .args(extra)
        .arg("--offline")
        .arg("--cache-dir")
        .arg(temp.join("cache"))
        .arg("--target-dir")
        .arg(temp.join("target"))
        .env_remove("MITTS_CACHE_DIR")
        .env_remove("CARGO_INCREMENTAL")
        .env_remove("CARGO_PROFILE_DEV_INCREMENTAL");
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "mitts build {extra:?} failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

/// Session directories rustc created for `crate_name` (`<crate>-<stable crate id>`).
fn sessions(incr: &Path, crate_name: &str) -> Vec<PathBuf> {
    let prefix = format!("{crate_name}-");
    let mut found: Vec<PathBuf> = std::fs::read_dir(incr)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.is_dir()
                        && p.file_name()
                            .and_then(|n| n.to_str())
                            .is_some_and(|n| n.starts_with(&prefix))
                })
                .collect()
        })
        .unwrap_or_default();
    found.sort();
    found
}

fn workspace(root: &Path, dev_profile: &str) {
    write(
        root,
        "Cargo.toml",
        &format!("[workspace]\nresolver='2'\nmembers=['core']\n{dev_profile}"),
    );
    write(
        root,
        "core/Cargo.toml",
        "[package]\nname='core_lib'\nversion='0.1.0'\nedition='2021'\n",
    );
    write(root, "core/src/lib.rs", "pub fn v() -> u32 { 1 }\n");
}

#[test]
fn dev_build_of_a_workspace_crate_is_incremental_and_reuses_its_session_after_an_edit() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("ws");
    workspace(&root, "");
    let incr = temp.path().join("target/debug/incremental");

    build(&root, temp.path(), &[], &[]);
    let first = sessions(&incr, "core_lib");
    assert_eq!(
        first.len(),
        1,
        "one incremental session after a dev build: {first:?}"
    );

    // An edit is a cache miss; rustc must find the same session, not open a second one.
    write(&root, "core/src/lib.rs", "pub fn v() -> u32 { 2 }\n");
    build(&root, temp.path(), &[], &[]);
    assert_eq!(
        sessions(&incr, "core_lib"),
        first,
        "the edit must reuse the crate's session (stable -C metadata)"
    );

    // A release build stays non-incremental, as in Cargo.
    build(&root, temp.path(), &["--release"], &[]);
    assert!(
        sessions(&temp.path().join("target/release/incremental"), "core_lib").is_empty(),
        "release profile must not be incremental"
    );
}

#[test]
fn incremental_can_be_turned_off_by_profile_and_by_cargo_incremental() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("ws");
    workspace(&root, "[profile.dev]\nincremental=false\n");
    build(&root, temp.path(), &[], &[]);
    assert!(sessions(&temp.path().join("target/debug/incremental"), "core_lib").is_empty());

    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("ws");
    workspace(&root, "");
    build(&root, temp.path(), &[], &[("CARGO_INCREMENTAL", "0")]);
    assert!(sessions(&temp.path().join("target/debug/incremental"), "core_lib").is_empty());

    // And on for a release profile that asks for it.
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("ws");
    workspace(&root, "[profile.release]\nincremental=true\n");
    build(&root, temp.path(), &["--release"], &[]);
    assert_eq!(
        sessions(&temp.path().join("target/release/incremental"), "core_lib").len(),
        1
    );
}
