/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! One cache root for every store. `--cache-dir`, `MITTS_CACHE_DIR` and
//! `XDG_CACHE_HOME` relocate the artifact store, the action cache, the locks,
//! the jobserver, the unpacked sources and the sparse-index cache together, so a
//! build never touches `~/.cache/mitts` unless that is the chosen root. The
//! binary runs with a throwaway HOME, which makes a leak into the default
//! location observable.

use mitts::adapter::{RustAdapter, RustBuildOptions};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

const STORES: [&str; 6] = ["cas", "ac", "locks", "jobserver", "sources", "index"];

fn copy_dir(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).unwrap();
    for entry in fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if entry.file_name() == "target" {
            continue;
        }
        let dest = dst.join(entry.file_name());
        if path.is_dir() {
            copy_dir(&path, &dest);
        } else {
            fs::copy(&path, &dest).unwrap();
        }
    }
}

/// A copy of `fixtures/hello-rust` without its `Cargo.lock`, so the build has to
/// resolve `itoa` through the sparse index and fetch it into the source store.
fn project_needing_resolution(root: &Path) -> PathBuf {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("hello-rust");
    let project = root.join("project");
    copy_dir(&src, &project);
    let _ = fs::remove_file(project.join("Cargo.lock"));
    project.join("Cargo.toml")
}

/// Throwaway HOME and XDG_CACHE_HOME for one `mitts` process; the real Cargo and
/// rustup homes are kept so the toolchain and registry stay reachable.
struct Sandbox {
    home: TempDir,
}

impl Sandbox {
    fn new() -> Self {
        Self {
            home: TempDir::new().unwrap(),
        }
    }

    fn default_store(&self) -> PathBuf {
        self.home.path().join(".cache").join("mitts")
    }

    fn xdg_store(&self) -> PathBuf {
        self.home.path().join("xdg").join("mitts")
    }

    fn mitts(&self) -> Command {
        let real_home = std::env::var_os("HOME").map(PathBuf::from);
        let cargo_home = std::env::var_os("CARGO_HOME")
            .map(PathBuf::from)
            .or_else(|| real_home.as_ref().map(|h| h.join(".cargo")))
            .expect("HOME or CARGO_HOME");
        let rustup_home = std::env::var_os("RUSTUP_HOME")
            .map(PathBuf::from)
            .or_else(|| real_home.as_ref().map(|h| h.join(".rustup")))
            .expect("HOME or RUSTUP_HOME");
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mitts"));
        cmd.env("HOME", self.home.path())
            .env("XDG_CACHE_HOME", self.home.path().join("xdg"))
            .env("CARGO_HOME", cargo_home)
            .env("RUSTUP_HOME", rustup_home)
            .env_remove("MITTS_CACHE_DIR");
        cmd
    }
}

fn stores_under(root: &Path) -> Vec<&'static str> {
    STORES
        .iter()
        .copied()
        .filter(|name| root.join(name).is_dir())
        .collect()
}

fn assert_build_ok(output: std::process::Output) {
    assert!(
        output.status.success(),
        "mitts build failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_every_store_under(root: &Path) {
    let present = stores_under(root);
    for name in ["cas", "ac", "sources", "index"] {
        assert!(
            present.contains(&name),
            "{name} missing under {}: found {present:?}",
            root.display()
        );
    }
}

#[test]
fn cache_dir_flag_relocates_every_store() {
    let sandbox = Sandbox::new();
    let work = TempDir::new().unwrap();
    let manifest = project_needing_resolution(work.path());
    let cache = work.path().join("cache");

    let output = sandbox
        .mitts()
        .arg("build")
        .arg("--manifest-path")
        .arg(&manifest)
        .arg("--cache-dir")
        .arg(&cache)
        .output()
        .unwrap();
    assert_build_ok(output);

    assert_every_store_under(&cache);
    assert!(
        !sandbox.default_store().exists(),
        "build leaked into ~/.cache/mitts"
    );
    assert!(
        !sandbox.xdg_store().exists(),
        "build leaked into $XDG_CACHE_HOME/mitts"
    );
}

#[test]
fn mitts_cache_dir_env_relocates_every_store() {
    let sandbox = Sandbox::new();
    let work = TempDir::new().unwrap();
    let manifest = project_needing_resolution(work.path());
    let cache = work.path().join("env_cache");

    let output = sandbox
        .mitts()
        .env("MITTS_CACHE_DIR", &cache)
        .arg("build")
        .arg("--manifest-path")
        .arg(&manifest)
        .output()
        .unwrap();
    assert_build_ok(output);

    assert_every_store_under(&cache);
    assert!(
        !sandbox.default_store().exists(),
        "build leaked into ~/.cache/mitts"
    );
    assert!(
        !sandbox.xdg_store().exists(),
        "build leaked into $XDG_CACHE_HOME/mitts"
    );
}

#[test]
fn xdg_cache_home_is_honoured_by_every_store() {
    let sandbox = Sandbox::new();
    let work = TempDir::new().unwrap();
    let manifest = project_needing_resolution(work.path());

    let output = sandbox
        .mitts()
        .arg("build")
        .arg("--manifest-path")
        .arg(&manifest)
        .output()
        .unwrap();
    assert_build_ok(output);

    assert_every_store_under(&sandbox.xdg_store());
    assert!(
        !sandbox.default_store().exists(),
        "a store ignored XDG_CACHE_HOME and fell back to ~/.cache/mitts"
    );
}

#[test]
fn explain_with_cache_dir_keeps_index_and_sources_under_it() {
    let sandbox = Sandbox::new();
    let work = TempDir::new().unwrap();
    let manifest = project_needing_resolution(work.path());
    let cache = work.path().join("explain_cache");

    let output = sandbox
        .mitts()
        .arg("explain")
        .arg("--manifest-path")
        .arg(&manifest)
        .arg("--cache-dir")
        .arg(&cache)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "mitts explain failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let present = stores_under(&cache);
    assert!(
        present.contains(&"index"),
        "index cache not under --cache-dir: {present:?}"
    );
    assert!(
        present.contains(&"sources"),
        "sources not under --cache-dir: {present:?}"
    );
    assert!(!sandbox.default_store().exists());
    assert!(!sandbox.xdg_store().exists());
}

fn dependency_free_project(root: &Path) -> PathBuf {
    let project = root.join("plain");
    fs::create_dir_all(project.join("src")).unwrap();
    fs::write(
        project.join("Cargo.toml"),
        "[package]\nname = \"plain\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n",
    )
    .unwrap();
    fs::write(project.join("src/lib.rs"), "pub fn answer() -> u8 { 42 }\n").unwrap();
    project.join("Cargo.toml")
}

#[test]
fn fast_lock_stays_active_with_explicit_cache_dir_only_when_asked() {
    let work = TempDir::new().unwrap();
    let manifest = dependency_free_project(work.path());
    let cache = work.path().join("cache");

    let asked = RustBuildOptions {
        manifest_path: Some(manifest.clone()),
        cache_dir: Some(cache.clone()),
        fast_lock: Some(true),
        ..Default::default()
    };
    let (_, first) = RustAdapter::build(&asked).unwrap();
    assert!(first.success, "{}", first.stderr);
    let (_, second) = RustAdapter::build(&asked).unwrap();
    assert!(second.success, "{}", second.stderr);
    assert_eq!(
        second.action_id, "fast-lock-hit",
        "fast lock must answer the no-op build when fast_lock is Some(true)"
    );

    let not_asked = RustBuildOptions {
        manifest_path: Some(manifest),
        target_dir: Some(work.path().join("other_target")),
        cache_dir: Some(cache),
        ..Default::default()
    };
    let (_, first) = RustAdapter::build(&not_asked).unwrap();
    assert!(first.success, "{}", first.stderr);
    let (_, second) = RustAdapter::build(&not_asked).unwrap();
    assert!(second.success, "{}", second.stderr);
    assert_ne!(
        second.action_id, "fast-lock-hit",
        "an explicit cache_dir alone keeps the fast lock off"
    );
}
