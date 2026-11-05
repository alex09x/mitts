/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! The shared cache had no size bound and nothing cleaned it (issue 52). After a command that
//! works with the cache, Mitts checks its size, at most once per interval, and prunes the oldest
//! objects when it is over `MITTS_CACHE_MAX_BYTES` (default 100 GiB).

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
        write(
            &dir.path().join("Cargo.toml"),
            "[package]\nname = \"sized\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        );
        write(&dir.path().join("src/lib.rs"), "pub fn f() -> u32 { 1 }\n");
        Project { dir }
    }

    fn cache(&self) -> PathBuf {
        self.dir.path().join("cache")
    }

    /// `mitts check` with the given environment for the cache limit.
    fn check(&self, env: &[(&str, &str)]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_mitts"));
        command
            .current_dir(self.dir.path())
            .arg("check")
            .arg("--cache-dir")
            .arg(self.cache())
            .env("CARGO_HOME", self.dir.path().join("cargo-home"))
            .env_remove("MITTS_CACHE_MAX_BYTES")
            .env_remove("MITTS_GC_INTERVAL_SECS")
            .env_remove("RUSTC_WRAPPER")
            .env_remove("CARGO_BUILD_RUSTC_WRAPPER");
        for (key, value) in env {
            command.env(key, value);
        }
        command.output().unwrap()
    }

    fn objects(&self) -> usize {
        fs::read_dir(self.cache().join("cas/objects"))
            .map(|entries| entries.flatten().count())
            .unwrap_or(0)
    }
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn assert_success(output: &Output) {
    assert!(output.status.success(), "{}", stderr(output));
}

#[test]
fn a_cache_over_the_limit_is_pruned_after_the_command() {
    let project = Project::new();
    let output = project.check(&[
        ("MITTS_CACHE_MAX_BYTES", "1"),
        ("MITTS_GC_INTERVAL_SECS", "0"),
    ]);
    assert_success(&output);
    assert_eq!(project.objects(), 0, "{}", stderr(&output));
    assert!(
        stderr(&output).contains("[mitts::gc]"),
        "{}",
        stderr(&output)
    );
    assert!(
        stderr(&output).contains("MITTS_CACHE_MAX_BYTES"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn the_build_after_a_pruning_recompiles_and_succeeds() {
    let project = Project::new();
    let limit = [
        ("MITTS_CACHE_MAX_BYTES", "1"),
        ("MITTS_GC_INTERVAL_SECS", "0"),
    ];
    assert_success(&project.check(&limit));
    let again = project.check(&limit);
    assert_success(&again);
    assert!(stderr(&again).contains("compiled=1"), "{}", stderr(&again));
}

#[test]
fn under_the_default_limit_nothing_is_pruned() {
    let project = Project::new();
    let output = project.check(&[("MITTS_GC_INTERVAL_SECS", "0")]);
    assert_success(&output);
    assert!(project.objects() > 0);
    assert!(
        !stderr(&output).contains("[mitts::gc]"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn off_and_zero_disable_the_limit() {
    for value in ["off", "0"] {
        let project = Project::new();
        let output = project.check(&[
            ("MITTS_CACHE_MAX_BYTES", value),
            ("MITTS_GC_INTERVAL_SECS", "0"),
        ]);
        assert_success(&output);
        assert!(project.objects() > 0, "{value}");
        assert!(
            !stderr(&output).contains("[mitts::gc]"),
            "{value}: {}",
            stderr(&output)
        );
    }
}

#[test]
fn the_size_is_checked_at_most_once_per_interval() {
    let project = Project::new();
    // The first command is the first check: it prunes.
    let first = project.check(&[("MITTS_CACHE_MAX_BYTES", "1")]);
    assert_success(&first);
    assert_eq!(project.objects(), 0, "{}", stderr(&first));
    // The second one, within the interval, does not check, and the new objects stay.
    let second = project.check(&[("MITTS_CACHE_MAX_BYTES", "1")]);
    assert_success(&second);
    assert!(project.objects() > 0, "{}", stderr(&second));
    assert!(
        !stderr(&second).contains("[mitts::gc]"),
        "{}",
        stderr(&second)
    );
}
