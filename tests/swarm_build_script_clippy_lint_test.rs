/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! `mitts clippy` lints build.rs like Cargo does. In a swarm of worktrees sharing one
//! cache, the processes that waited for another worktree's build-script run took an
//! early exit that skipped the lint, so the warning in build.rs was missing from their
//! output and appeared only on the next, fully cached run (ring's clippy swarm).

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use tempfile::TempDir;

fn write_project(dir: &Path) {
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\nbuild = \"build.rs\"\n",
    )
    .unwrap();
    fs::write(
        dir.join("build.rs"),
        "fn main() { let _lint_me = \"a\".len() == 0; println!(\"cargo:rerun-if-changed=build.rs\"); }\n",
    )
    .unwrap();
    fs::write(dir.join("src/lib.rs"), "pub fn v() -> u32 { 1 }\n").unwrap();
}

#[test]
fn every_worktree_of_a_clippy_swarm_reports_the_build_script_lint() {
    let cache = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    let dirs: Vec<_> = (0..4)
        .map(|i| {
            let d = root.path().join(format!("wt{i}"));
            write_project(&d);
            d
        })
        .collect();
    let children: Vec<_> = dirs
        .iter()
        .map(|d| {
            Command::new(env!("CARGO_BIN_EXE_mitts"))
                .args(["clippy", "--manifest-path"])
                .arg(d.join("Cargo.toml"))
                .arg("--cache-dir")
                .arg(cache.path())
                .env("CI", "true")
                .env_remove("MITTS_CACHE_DIR")
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("spawn mitts")
        })
        .collect();
    for (i, child) in children.into_iter().enumerate() {
        let out = child.wait_with_output().unwrap();
        let text = format!(
            "{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(out.status.success(), "worktree {i} lints:\n{text}");
        assert!(
            text.contains("length comparison to zero"),
            "worktree {i} shows the build.rs lint whichever process ran the script:\n{text}"
        );
    }
}
