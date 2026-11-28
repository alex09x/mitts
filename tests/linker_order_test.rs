/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Which linker links a binary when both `target.<host>.linker` and a `-C linker` in
//! RUSTFLAGS are set (issue #143, a Fedora riscv64 build): as with Cargo, RUSTFLAGS win,
//! with link timing on or off, and the summary names the linker that really ran.
//! `gcc` and `clang` are stand-in scripts that record their name and run `cc`.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

fn write_file(path: impl AsRef<Path>, contents: &str) {
    let path = path.as_ref();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn host() -> String {
    let out = Command::new("rustc")
        .arg("-vV")
        .output()
        .expect("rustc -vV");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find_map(|l| l.strip_prefix("host: "))
        .expect("host line")
        .trim()
        .to_string()
}

/// A binary project whose config names `gcc`, and a bin dir with `gcc` and `clang`
/// stand-ins that append their name to `linked.log` before running `cc`.
fn project() -> TempDir {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    write_file(
        root.join("p/Cargo.toml"),
        "[package]\nname = \"lk\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write_file(root.join("p/src/main.rs"), "fn main() {}\n");
    write_file(
        root.join("p/.cargo/config.toml"),
        &format!("[target.{}]\nlinker = \"gcc\"\n", host()),
    );
    let log = root.join("linked.log");
    for name in ["gcc", "clang"] {
        let script = root.join("bin").join(name);
        // `--version` is the summary asking for the linker's version, not a link.
        write_file(
            &script,
            &format!(
                "#!/bin/sh\n[ \"$1\" = \"--version\" ] || echo {name} >> '{}'\nexec cc \"$@\"\n",
                log.display()
            ),
        );
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    }
    temp
}

/// Builds the project and returns the linkers that ran and the build's stdout.
fn build(temp: &TempDir, rustflags: Option<&str>, link_timing: Option<&str>) -> (String, String) {
    let root = temp.path();
    let _ = fs::remove_file(root.join("linked.log"));
    let path = format!(
        "{}:{}",
        root.join("bin").display(),
        std::env::var("PATH").unwrap()
    );
    // A fresh cache each time: a binary restored from the cache is not linked at all.
    static RUN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let run = RUN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mitts"));
    cmd.args(["build", "--cache-dir"])
        .arg(root.join(format!("mitts-cache-{run}")))
        .current_dir(root.join("p"))
        .env("PATH", path)
        .env("RUSTC_WRAPPER", "")
        .env("MITTS_FAST_LOCK", "0")
        .env("CARGO_TARGET_DIR", root.join("target"))
        .env_remove("CARGO_BUILD_TARGET_DIR")
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .env_remove("MITTS_VERBOSE")
        .env_remove("RUSTFLAGS")
        .env_remove("MITTS_LINK_TIMING");
    if let Some(flags) = rustflags {
        cmd.env("RUSTFLAGS", flags);
    }
    if let Some(value) = link_timing {
        cmd.env("MITTS_LINK_TIMING", value);
    }
    let out = cmd.output().expect("run Mitts");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        out.status.success(),
        "stdout:\n{stdout}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let linked = fs::read_to_string(root.join("linked.log")).unwrap_or_default();
    let _ = fs::remove_dir_all(root.join("target"));
    (linked.trim().to_string(), stdout)
}

#[test]
fn a_rustflags_linker_wins_over_the_configured_one() {
    let temp = project();
    for flags in ["-C linker=clang", "-Clinker=clang"] {
        let (linked, stdout) = build(&temp, Some(flags), None);
        assert_eq!(linked, "clang", "RUSTFLAGS={flags}\n{stdout}");
        // The wrapper measured the RUSTFLAGS linker, so the summary names it.
        assert!(
            stdout.contains("linker: clang"),
            "RUSTFLAGS={flags}\n{stdout}"
        );
        let (linked, stdout) = build(&temp, Some(flags), Some("0"));
        assert_eq!(
            linked, "clang",
            "MITTS_LINK_TIMING=0 RUSTFLAGS={flags}\n{stdout}"
        );
    }
}

#[test]
fn the_configured_linker_is_used_without_a_rustflags_one() {
    let temp = project();
    let (linked, stdout) = build(&temp, None, None);
    assert_eq!(linked, "gcc", "{stdout}");
    let (linked, stdout) = build(&temp, Some("-C opt-level=1"), Some("0"));
    assert_eq!(linked, "gcc", "{stdout}");
}

#[test]
fn a_linker_flavor_turns_the_wrapper_off_but_rustflags_still_win() {
    let temp = project();
    let (linked, stdout) = build(&temp, Some("-C linker=clang -C linker-flavor=gcc"), None);
    assert_eq!(linked, "clang", "{stdout}");
    assert!(!stdout.contains("linker: "), "not measured:\n{stdout}");
}
