/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! rustc runs LTO only for executables, cdylibs and staticlibs and rejects `-C lto` when one
//! unit also emits an rlib. Cargo passes no LTO flag to such a mixed library; Mitts must build
//! it with the profile's `lto` setting present and the requested outputs produced.

#![cfg(unix)]

use mitts::adapter::{RustAdapter, RustBuildOptions};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

const MIXED: &str = "\"staticlib\", \"cdylib\", \"rlib\"";

struct Project {
    dir: TempDir,
    wrapper_log: PathBuf,
}

/// A one-function library named `tako_core` with the given `crate-type` list and release
/// `lto` value. With `with_wrapper` every rustc invocation is logged by a wrapper; the wrapper
/// path is part of the cache key, so the cache tests build without it.
fn project(crate_types: &str, lto: &str, with_wrapper: bool) -> Project {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(root.join(".cargo")).unwrap();
    fs::write(root.join("src/lib.rs"), "pub fn f() -> u32 { 1 }\n").unwrap();
    fs::write(
        root.join("Cargo.toml"),
        format!(
            "[package]\nname = \"tako_core_demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
             [lib]\nname = \"tako_core\"\ncrate-type = [{crate_types}]\n\n\
             [profile.release]\nopt-level = 3\nlto = {lto}\n"
        ),
    )
    .unwrap();
    let wrapper_log = root.join("rustc-calls.log");
    if with_wrapper {
        let wrapper = root.join("wrap.sh");
        fs::write(
            &wrapper,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nexec \"$@\"\n",
                wrapper_log.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(
            root.join(".cargo/config.toml"),
            format!("[build]\nrustc-wrapper = \"{}\"\n", wrapper.display()),
        )
        .unwrap();
    }
    Project { dir, wrapper_log }
}

fn build_release_lib(project: &Project, cache: &Path) -> (i32, bool) {
    let opt = RustBuildOptions {
        manifest_path: Some(project.dir.path().join("Cargo.toml")),
        // Cargo config (the rustc wrapper) is discovered from here, as the CLI does from cwd.
        start_dir: Some(project.dir.path().to_path_buf()),
        cache_dir: Some(cache.to_path_buf()),
        release: true,
        lib_only: true,
        ..Default::default()
    };
    let (_, result) = RustAdapter::build_native(&opt).expect("build failed");
    (result.exit_code, result.cached)
}

/// The real CLI, run in the project directory like a user would, so Cargo config (the rustc
/// wrapper) is discovered from the project.
fn build_release_lib_with_cli(project: &Project, cache: &Path) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_mitts"))
        .current_dir(project.dir.path())
        .args(["build", "--release", "--lib", "--offline", "--cache-dir"])
        .arg(cache)
        .env("CARGO_HOME", project.dir.path().join("cargo-home"))
        .env_remove("RUSTC_WRAPPER")
        .env_remove("RUSTC_WORKSPACE_WRAPPER")
        .env_remove("CARGO_BUILD_RUSTC_WRAPPER")
        .env_remove("CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER")
        .output()
        .unwrap()
}

/// rustc command lines the wrapper saw for the crate `tako_core`.
fn tako_core_calls(project: &Project) -> Vec<String> {
    fs::read_to_string(&project.wrapper_log)
        .unwrap_or_default()
        .lines()
        .filter(|line| line.contains("--crate-name tako_core"))
        .map(str::to_string)
        .collect()
}

/// A non-empty `libtako_core*` output with one of `extensions` in the release directory or its
/// `deps` directory (Mitts keeps some library kinds only under `deps`).
fn has_nonempty_output(project: &Project, extensions: &[&str]) -> bool {
    let release = project.dir.path().join("target/release");
    [release.clone(), release.join("deps")]
        .iter()
        .filter_map(|dir| fs::read_dir(dir).ok())
        .flat_map(|entries| entries.flatten())
        .any(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            name.starts_with("libtako_core")
                && extensions.iter().any(|ext| name.ends_with(ext))
                && entry.metadata().map(|m| m.len() > 0).unwrap_or(false)
        })
}

#[test]
fn mixed_crate_type_library_builds_under_fat_and_thin_lto() {
    for lto in ["true", "\"thin\""] {
        let cache = TempDir::new().unwrap();
        let project = project(MIXED, lto, true);
        let output = build_release_lib_with_cli(&project, cache.path());
        assert!(
            output.status.success(),
            "lto = {lto} must build a mixed library: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        assert!(
            has_nonempty_output(&project, &[".a"]),
            "static library missing for lto = {lto}"
        );
        assert!(
            has_nonempty_output(&project, &[".rlib"]),
            "rlib missing for lto = {lto}"
        );
        assert!(
            has_nonempty_output(&project, &[".so", ".dylib"]),
            "shared library missing for lto = {lto}"
        );

        let calls = tako_core_calls(&project);
        assert!(
            !calls.is_empty(),
            "wrapper never saw tako_core (lto = {lto})"
        );
        for call in &calls {
            assert!(
                !call.contains("lto="),
                "a unit that also emits an rlib must get no -C lto flag, like Cargo: {call}"
            );
        }
    }
}

#[test]
fn mixed_crate_type_library_is_a_full_cache_hit_on_rebuild() {
    let cache = TempDir::new().unwrap();
    let first = project(MIXED, "true", false);
    let (code, cached) = build_release_lib(&first, cache.path());
    assert_eq!(code, 0);
    assert!(!cached, "first build compiles");

    let second = project(MIXED, "true", false);
    let (code, cached) = build_release_lib(&second, cache.path());
    assert_eq!(code, 0);
    assert!(
        cached,
        "a fresh clone against the same cache restores everything"
    );
}

#[test]
fn lto_setting_still_reaches_the_cache_key() {
    let cache = TempDir::new().unwrap();
    let with_lto = project(MIXED, "true", false);
    assert_eq!(build_release_lib(&with_lto, cache.path()).0, 0);

    let without_lto = project(MIXED, "false", false);
    let (code, cached) = build_release_lib(&without_lto, cache.path());
    assert_eq!(code, 0);
    assert!(
        !cached,
        "changing lto must not reuse the other setting's artifacts"
    );
}

#[test]
fn cdylib_only_library_still_runs_lto() {
    let cache = TempDir::new().unwrap();
    let project = project("\"cdylib\"", "true", true);
    let output = build_release_lib_with_cli(&project, cache.path());
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(has_nonempty_output(&project, &[".so", ".dylib"]));
    let calls = tako_core_calls(&project);
    assert!(
        calls.iter().any(|call| call.contains("lto=fat")),
        "a cdylib-only unit keeps -C lto=fat: {calls:?}"
    );
}
