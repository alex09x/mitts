/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A build without the cache (`--no-cache`, or a configured rustc wrapper) stored its outputs in
//! the shared content-addressed store under names that belong to one build, and nothing read them
//! again: each run added objects for good, and a build host filled its disk (issue 53). Such a
//! build now keeps its outputs in a private store next to the shared one that the next build or
//! `mitts gc` removes.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;

fn write(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

struct Project {
    dir: TempDir,
}

impl Project {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        write(
            &root.join("dep/Cargo.toml"),
            "[package]\nname = \"dep\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        );
        write(&root.join("dep/src/lib.rs"), "pub fn f() -> u32 { 1 }\n");
        write(
            &root.join("app/Cargo.toml"),
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
             [dependencies]\ndep = { path = \"../dep\" }\n",
        );
        write(
            &root.join("app/src/lib.rs"),
            "pub fn g() -> u32 { dep::f() }\n",
        );
        Project { dir }
    }

    fn cache(&self) -> PathBuf {
        self.dir.path().join("cache")
    }

    fn mitts(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_mitts"))
            .current_dir(self.dir.path().join("app"))
            .args(args)
            .arg("--cache-dir")
            .arg(self.cache())
            .env("CARGO_HOME", self.dir.path().join("cargo-home"))
            .env_remove("RUSTC_WRAPPER")
            .env_remove("CARGO_BUILD_RUSTC_WRAPPER")
            .output()
            .unwrap()
    }

    fn shared_objects(&self) -> usize {
        fs::read_dir(self.cache().join("cas/objects"))
            .map(|entries| entries.flatten().count())
            .unwrap_or(0)
    }

    fn private_stores(&self) -> usize {
        fs::read_dir(self.cache())
            .map(|entries| {
                entries
                    .flatten()
                    .filter(|e| {
                        e.file_name()
                            .to_string_lossy()
                            .starts_with("cas-ephemeral-")
                    })
                    .count()
            })
            .unwrap_or(0)
    }
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn an_uncached_build_leaves_the_shared_store_alone_and_does_not_pile_up() {
    let project = Project::new();
    for run in 1..=3 {
        assert_success(&project.mitts(&["check", "--no-cache"]));
        assert_eq!(
            project.shared_objects(),
            0,
            "run {run}: nothing of an uncached build is written to the shared store"
        );
        assert_eq!(
            project.private_stores(),
            1,
            "run {run}: the private store of the previous run is removed by the next build"
        );
    }
}

#[test]
fn gc_removes_the_private_store_of_a_finished_build() {
    let project = Project::new();
    assert_success(&project.mitts(&["check", "--no-cache"]));
    assert_eq!(project.private_stores(), 1);
    let gc = Command::new(env!("CARGO_BIN_EXE_mitts"))
        .arg("gc")
        .arg("--cache-dir")
        .arg(project.cache())
        .output()
        .unwrap();
    assert_success(&gc);
    assert!(
        String::from_utf8_lossy(&gc.stdout).contains("removed 1 private store"),
        "{}",
        String::from_utf8_lossy(&gc.stdout)
    );
    assert_eq!(project.private_stores(), 0);
}

#[test]
fn a_cached_build_still_uses_the_shared_store() {
    let project = Project::new();
    assert_success(&project.mitts(&["check"]));
    let objects = project.shared_objects();
    assert!(objects > 0, "a cached build stores its outputs");
    assert_eq!(project.private_stores(), 0);
    assert_success(&project.mitts(&["check"]));
    assert_eq!(
        project.shared_objects(),
        objects,
        "the repeat is a cache hit"
    );
}
