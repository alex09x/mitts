/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Profile `split-debuginfo` (zed sets `unpacked` in `[profile.dev]` and its build override).
//! Like Cargo: rustc gets `-C split-debuginfo=<v>` when debug info is on and the target
//! supports the value, Apple targets default to `unpacked`, and the debug-info files rustc
//! writes beside its outputs are where Cargo leaves them, after a cache hit too.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;

fn write_file(path: impl AsRef<Path>, contents: &str) {
    let path = path.as_ref();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// A binary with a library dependency, `split-debuginfo` set to `split` (or left out).
fn project(root: &Path, split: Option<&str>) {
    let profile = split
        .map(|s| format!("[profile.dev]\nsplit-debuginfo = \"{s}\"\n"))
        .unwrap_or_default();
    write_file(
        root.join("Cargo.toml"),
        &format!(
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\
             [dependencies]\nhelper = {{ path = \"helper\" }}\n{profile}"
        ),
    );
    write_file(
        root.join("src/main.rs"),
        "fn main() { println!(\"{}\", helper::answer()); }\n",
    );
    write_file(
        root.join("helper/Cargo.toml"),
        "[package]\nname = \"helper\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write_file(
        root.join("helper/src/lib.rs"),
        "pub fn answer() -> u32 { 42 }\n",
    );
}

/// Variables of the environment the tests run in that would change what Mitts builds.
const CLEARED: [&str; 10] = [
    "CARGO_TARGET_DIR",
    "CARGO_BUILD_TARGET_DIR",
    "CARGO_BUILD_TARGET",
    "CARGO_ENCODED_RUSTFLAGS",
    "RUSTFLAGS",
    "CARGO_BUILD_RUSTFLAGS",
    "RUSTC_WRAPPER",
    "RUSTC_WORKSPACE_WRAPPER",
    "CARGO_BUILD_RUSTC_WRAPPER",
    "CARGO_PROFILE_DEV_SPLIT_DEBUGINFO",
];

fn mitts(root: &Path, cache: &Path, target: &Path, args: &[&str]) -> Output {
    mitts_with(root, cache, target, args, None)
}

fn mitts_with(
    root: &Path,
    cache: &Path,
    target: &Path,
    args: &[&str],
    rustflags: Option<&str>,
) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mitts"));
    cmd.env("MITTS_SHARE_WORKSPACE_CRATES", "1")
        .args(args)
        .arg("--cache-dir")
        .arg(cache)
        .arg("--target-dir")
        .arg(target)
        .current_dir(root)
        .env("CARGO_HOME", root.join("cargo-home"))
        .env_remove("MITTS_CACHE_DIR");
    for name in CLEARED {
        cmd.env_remove(name);
    }
    if let Some(flags) = rustflags {
        cmd.env("RUSTFLAGS", flags);
    }
    cmd.output().expect("run Mitts")
}

/// Cargo passes the profile's flag before RUSTFLAGS, so RUSTFLAGS decides, and whatever
/// rustc wrote is kept: `off` there drops the profile's `packed`; `unpacked` there with
/// nothing in the profile still keeps its `.dwo` files, cold and after a hit.
#[test]
fn rustflags_decide_over_the_profile_like_cargo() {
    if cfg!(target_os = "macos") || !supported().iter().any(|v| v == "unpacked") {
        return;
    }
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("proj");
    project(&root, Some("packed"));
    let cache = temp.path().join("cache");
    let target = temp.path().join("target");
    let flags = "-C split-debuginfo=off";
    let out = mitts_with(&root, &cache, &target, &["build", "-vv"], Some(flags));
    let log = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(out.status.success(), "{log}");
    let packed = log
        .find("split-debuginfo=packed")
        .expect("the profile's flag");
    let off = log
        .rfind("split-debuginfo=off")
        .expect("the RUSTFLAGS flag");
    assert!(
        packed < off,
        "RUSTFLAGS come after the profile's flag: {log}"
    );
    assert!(!target.join("debug/app.dwp").exists(), "off: no package");

    let temp = TempDir::new().unwrap();
    let root = temp.path().join("proj");
    project(&root, None);
    let cache = temp.path().join("cache");
    let target = temp.path().join("target");
    let flags = Some("-C split-debuginfo=unpacked");
    let out = mitts_with(&root, &cache, &target, &["build"], flags);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let cold = files_with_ext(&target.join("debug/deps"), "dwo").len();
    assert!(cold > 0, "RUSTFLAGS' unpacked keeps the .dwo files");
    fs::remove_dir_all(&target).unwrap();
    let out = mitts_with(&root, &cache, &target, &["build", "-v"], flags);
    let log = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        out.status.success() && log.contains("compiled=0 cached=2"),
        "{log}"
    );
    assert_eq!(
        files_with_ext(&target.join("debug/deps"), "dwo").len(),
        cold
    );
}

/// The values rustc supports for this host.
fn supported() -> Vec<String> {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let out = Command::new(rustc)
        .args(["--print", "split-debuginfo"])
        .output()
        .expect("rustc --print split-debuginfo");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.trim().to_string())
        .collect()
}

fn files_with_ext(dir: &Path, ext: &str) -> Vec<PathBuf> {
    fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == ext))
                .collect()
        })
        .unwrap_or_default()
}

/// What a build left in the target directory.
struct Built {
    dir: PathBuf,
    /// Debug-info file names in `debug/deps` with extension `ext`.
    files: Vec<std::ffi::OsString>,
    dwp_beside_binary: bool,
    binary: Vec<u8>,
}

fn inspect(dir: &Path, ext: &str) -> Built {
    let app = dir.join(format!("debug/app{}", std::env::consts::EXE_SUFFIX));
    let run = Command::new(&app).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "42");
    let mut files: Vec<_> = files_with_ext(&dir.join("debug/deps"), ext)
        .iter()
        .map(|p| p.file_name().unwrap().to_owned())
        .collect();
    files.sort();
    Built {
        dir: dir.to_path_buf(),
        files,
        dwp_beside_binary: dir.join("debug/app.dwp").is_file(),
        binary: fs::read(&app).unwrap(),
    }
}

/// Builds cold, deletes the target directory and builds again from the same cache (all
/// hits, the same target path), returning both builds and the cold build's verbose log.
fn build_twice(split: Option<&str>, ext: &str) -> (TempDir, Built, Built, String) {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("proj");
    project(&root, split);
    let cache = temp.path().join("cache");
    let target = temp.path().join("target");
    let out = mitts(&root, &cache, &target, &["build", "-vv"]);
    let log = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(out.status.success(), "{log}");
    let cold = inspect(&target, ext);
    fs::remove_dir_all(&target).unwrap();
    let out = mitts(&root, &cache, &target, &["build", "-v"]);
    let warm_log = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(out.status.success(), "{warm_log}");
    assert!(
        warm_log.contains("compiled=0 cached=2"),
        "the second build is all cache hits: {warm_log}"
    );
    let warm = inspect(&target, ext);
    (temp, cold, warm, log)
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

#[test]
fn unpacked_split_debuginfo_matches_cargo_and_survives_a_cache_hit() {
    if !supported().iter().any(|v| v == "unpacked") {
        return;
    }
    // Linux keeps `.dwo` objects, macOS the object files the debug map points at.
    let ext = if cfg!(target_os = "macos") {
        "o"
    } else {
        "dwo"
    };
    let (_temp, cold, warm, log) = build_twice(Some("unpacked"), ext);
    assert!(log.contains("split-debuginfo=unpacked"), "{log}");
    assert!(!cold.files.is_empty(), "no .{ext} files in {:?}", cold.dir);
    assert_eq!(
        cold.files, warm.files,
        "a cache hit restores the same debug-info files"
    );
    if !cfg!(target_os = "macos") {
        // The objects name their `.dwo` files where they are, as with Cargo.
        let deps = cold.dir.join("debug/deps");
        for built in [&cold, &warm] {
            assert!(
                contains(&built.binary, deps.to_string_lossy().as_bytes()),
                "the executable names the .dwo files under {deps:?}"
            );
            assert!(!contains(&built.binary, b"/mitts/out/app/app."));
        }
    }
}

#[test]
fn packed_split_debuginfo_puts_the_package_beside_the_executable() {
    if cfg!(target_os = "macos")
        || cfg!(target_env = "msvc")
        || !supported().iter().any(|v| v == "packed")
    {
        // macOS `packed` runs dsymutil; covered by the unit tests of the placement. MSVC's
        // `packed` is the linker's `.pdb`, not a `.dwp`.
        return;
    }
    let (_temp, cold, warm, log) = build_twice(Some("packed"), "dwp");
    assert!(log.contains("split-debuginfo=packed"), "{log}");
    for built in [&cold, &warm] {
        assert!(
            built.dwp_beside_binary,
            "{:?}: the executable's .dwp, cold and after a hit",
            built.dir
        );
    }
}

#[test]
fn without_the_setting_only_apple_targets_split_debug_info() {
    let (_temp, _cold, _warm, log) = build_twice(None, "o");
    assert_eq!(
        log.contains("split-debuginfo="),
        cfg!(target_vendor = "apple"),
        "Cargo defaults to unpacked on Apple targets only: {log}"
    );
}

#[test]
fn split_debuginfo_with_debug_info_off_passes_no_flag() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("proj");
    project(&root, Some("unpacked"));
    let manifest = fs::read_to_string(root.join("Cargo.toml")).unwrap();
    fs::write(root.join("Cargo.toml"), format!("{manifest}debug = 0\n")).unwrap();
    let out = mitts(
        &root,
        &temp.path().join("cache"),
        &temp.path().join("target"),
        &["build", "-vv"],
    );
    let log = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{log}");
    assert!(!log.contains("split-debuginfo="), "{log}");
}

#[test]
fn an_unknown_split_debuginfo_value_is_refused() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("proj");
    project(&root, Some("bogus"));
    let out = mitts(
        &root,
        &temp.path().join("cache"),
        &temp.path().join("target"),
        &["build"],
    );
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("split-debuginfo"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// `mitts rustc -- -C split-debuginfo=…` reaches the primary target only; a dependency's
/// key must not change with it (it would rebuild with a key naming a value it never got).
/// MSVC accepts only `packed`, so the `off` this test passes is refused there (by rustc,
/// under Cargo too).
#[cfg(not(target_env = "msvc"))]
#[test]
fn a_command_line_split_flag_does_not_change_a_dependency_key() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("proj");
    project(&root, None);
    // `mitts rustc` builds the library target.
    write_file(
        root.join("src/lib.rs"),
        "pub fn answer() -> u32 { helper::answer() }\n",
    );
    let cache = temp.path().join("cache");
    let target = temp.path().join("target");
    let first = mitts(&root, &cache, &target, &["build"]);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );

    // The options go before `--`: everything after it is rustc's.
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mitts"));
    cmd.env("MITTS_SHARE_WORKSPACE_CRATES", "1")
        .args(["rustc", "--lib", "-vv", "--cache-dir"])
        .arg(&cache)
        .arg("--target-dir")
        .arg(&target)
        .args(["--", "-C", "split-debuginfo=off"])
        .current_dir(&root)
        .env("CARGO_HOME", root.join("cargo-home"))
        .env_remove("MITTS_CACHE_DIR");
    for name in CLEARED {
        cmd.env_remove(name);
    }
    let out = cmd.output().unwrap();
    let log = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(out.status.success(), "{log}");
    assert!(
        !log.contains("--crate-name helper "),
        "the dependency is reused, not recompiled: {log}"
    );
    assert!(
        log.contains("--crate-name app "),
        "the primary target is rebuilt: {log}"
    );
}
