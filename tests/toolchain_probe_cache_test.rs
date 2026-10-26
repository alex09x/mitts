/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! The toolchain probes (`rustup which rustc`, `rustc -vV`, `rustc --print cfg`) run once
//! per compiler, not once per Mitts invocation. Wrapper scripts stand in for rustc and
//! rustup and log every real spawn, so the tests count processes, not cache files.

#![cfg(unix)]

use mitts::adapter::rust::toolchain_probes::{
    file_identity, is_rustup_proxy, load_or_probe, rustup_which_rustc, set_cache_root,
};
use std::cell::Cell;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};
use tempfile::TempDir;

fn real_rustc() -> PathBuf {
    if let Ok(out) = Command::new("rustup").args(["which", "rustc"]).output() {
        if out.status.success() {
            let p = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
            if p.is_file() {
                return p;
            }
        }
    }
    std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|d| d.join("rustc"))
        .find(|p| p.is_file())
        .expect("a rustc on PATH")
}

fn write_script(path: &Path, body: &str) {
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

/// A `rustc` that appends its arguments to `log` and runs the real compiler.
fn rustc_wrapper(dir: &Path, log: &Path) -> PathBuf {
    let wrapper = dir.join("rustc");
    write_script(
        &wrapper,
        &format!(
            "#!/bin/sh\necho \"$*\" >> '{}'\nexec '{}' \"$@\"\n",
            log.display(),
            real_rustc().display()
        ),
    );
    wrapper
}

/// A binary with a `cfg(unix)` path dependency: the target-specific dependency makes the
/// planner evaluate the cfg, which is what asks rustc for its cfg atoms.
fn write_project(dir: &Path) -> PathBuf {
    fs::create_dir_all(dir.join("app/src")).unwrap();
    fs::create_dir_all(dir.join("helper/src")).unwrap();
    fs::write(
        dir.join("Cargo.toml"),
        "[workspace]\nmembers = [\"app\", \"helper\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    fs::write(
        dir.join("app/Cargo.toml"),
        "[package]\nname = \"probe_app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[target.'cfg(unix)'.dependencies]\nhelper = { path = \"../helper\" }\n",
    )
    .unwrap();
    fs::write(
        dir.join("app/src/main.rs"),
        "fn main() { helper::hello(); }\n",
    )
    .unwrap();
    fs::write(
        dir.join("helper/Cargo.toml"),
        "[package]\nname = \"helper\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(dir.join("helper/src/lib.rs"), "pub fn hello() {}\n").unwrap();
    dir.join("Cargo.toml")
}

fn mitts_build(manifest: &Path, cache: &Path, env: &[(&str, &str)], drop_rustc: bool) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mitts"));
    cmd.arg("build")
        .arg("--manifest-path")
        .arg(manifest)
        .arg("--cache-dir")
        .arg(cache)
        .current_dir(manifest.parent().unwrap())
        .env("CI", "true")
        .env_remove("MITTS_CACHE_DIR");
    if drop_rustc {
        cmd.env_remove("RUSTC");
    }
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd.output().expect("run mitts");
    assert!(
        out.status.success(),
        "mitts build failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn count_lines(log: &Path, pred: impl Fn(&str) -> bool) -> usize {
    fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .filter(|l| pred(l))
        .count()
}

fn bump_mtime(path: &Path) {
    let later = SystemTime::now() + Duration::from_secs(10);
    fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(later)
        .unwrap();
}

#[test]
fn probe_cache_rustc_version_and_cfg_run_once_per_compiler_binary() {
    let temp = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let log = temp.path().join("rustc.log");
    let wrapper = rustc_wrapper(temp.path(), &log);
    let manifest = write_project(&temp.path().join("app"));
    let env = [("RUSTC", wrapper.to_str().unwrap())];
    let vv = |l: &str| l == "-vV";
    let cfg = |l: &str| l.starts_with("--print cfg");

    mitts_build(&manifest, cache.path(), &env, false);
    assert!(count_lines(&log, vv) >= 1, "first build must probe -vV");
    assert!(
        count_lines(&log, cfg) >= 1,
        "first build must probe --print cfg"
    );
    let entries: Vec<_> = fs::read_dir(cache.path().join("toolchain"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        entries.iter().any(|n| n.starts_with("rustc-vV-"))
            && entries.iter().any(|n| n.starts_with("rustc-cfg-")),
        "probe entries missing: {entries:?}"
    );

    let _ = fs::remove_file(&log); // absent when nothing was spawned at all
    mitts_build(&manifest, cache.path(), &env, false);
    assert_eq!(count_lines(&log, vv), 0, "second build spawned rustc -vV");
    assert_eq!(
        count_lines(&log, cfg),
        0,
        "second build spawned rustc --print cfg"
    );

    // A changed compiler binary (rustup update) is probed again.
    bump_mtime(&wrapper);
    let _ = fs::remove_file(&log); // absent when nothing was spawned at all
    mitts_build(&manifest, cache.path(), &env, false);
    assert!(
        count_lines(&log, vv) >= 1,
        "a changed rustc must be probed again"
    );
    assert!(count_lines(&log, cfg) >= 1);
}

#[test]
fn probe_cache_rustup_which_runs_once_per_rustup_and_selection() {
    let temp = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let log = temp.path().join("rustup.log");
    let bin = temp.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    write_script(
        &bin.join("rustup"),
        &format!(
            "#!/bin/sh\necho \"$*\" >> '{}'\nif [ \"$1\" = which ] && [ \"$2\" = rustc ]; then echo '{}'; exit 0; fi\nexit 1\n",
            log.display(),
            real_rustc().display()
        ),
    );
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let manifest = write_project(&temp.path().join("app"));
    let env = [("PATH", path.as_str())];
    let which = |l: &str| l == "which rustc";

    mitts_build(&manifest, cache.path(), &env, true);
    assert_eq!(
        count_lines(&log, which),
        1,
        "first build resolves rustc once"
    );

    let _ = fs::remove_file(&log); // absent when nothing was spawned at all
    mitts_build(&manifest, cache.path(), &env, true);
    assert_eq!(
        count_lines(&log, which),
        0,
        "second build spawned rustup which"
    );

    // A different toolchain selection is a different answer.
    fs::write(
        temp.path().join("app/rust-toolchain.toml"),
        "[toolchain]\nchannel = \"stable\"\n",
    )
    .unwrap();
    let _ = fs::remove_file(&log); // absent when nothing was spawned at all
    mitts_build(&manifest, cache.path(), &env, true);
    assert_eq!(
        count_lines(&log, which),
        1,
        "a rust-toolchain file must re-resolve"
    );
}

#[test]
fn probe_cache_primitives_follow_key_identity_and_tolerate_damage() {
    let cache = TempDir::new().unwrap();
    set_cache_root(cache.path());
    let calls = Cell::new(0u32);
    let probe = || -> Result<String, ()> {
        calls.set(calls.get() + 1);
        Ok(format!("answer{}", calls.get()))
    };

    assert_eq!(load_or_probe("t", "k1", true, probe).unwrap(), "answer1");
    assert_eq!(load_or_probe("t", "k1", true, probe).unwrap(), "answer1");
    assert_eq!(calls.get(), 1, "a cached key must not probe");
    assert_eq!(load_or_probe("t", "k2", true, probe).unwrap(), "answer2");
    assert_eq!(load_or_probe("t", "k1", false, probe).unwrap(), "answer3");
    assert_eq!(calls.get(), 3, "an uncacheable probe always runs");
    assert!(load_or_probe("t", "k3", true, || Err::<String, ()>(())).is_err());
    assert_eq!(
        load_or_probe("t", "k3", true, probe).unwrap(),
        "answer4",
        "a failure is not stored"
    );

    // Damaged entries are replaced, not trusted.
    let dir = cache.path().join("toolchain");
    for entry in fs::read_dir(&dir).unwrap().flatten() {
        fs::write(entry.path(), b"{ not json").unwrap();
    }
    assert_eq!(load_or_probe("t", "k1", true, probe).unwrap(), "answer5");
    assert_eq!(load_or_probe("t", "k1", true, probe).unwrap(), "answer5");

    // File identity moves with the mtime; a hardlinked rustup proxy is recognised.
    let f = cache.path().join("rustc");
    fs::write(&f, b"x").unwrap();
    let before = file_identity(&f);
    bump_mtime(&f);
    assert_ne!(before, file_identity(&f));
    assert!(!is_rustup_proxy(&f));
    fs::hard_link(&f, cache.path().join("rustup")).unwrap();
    assert!(is_rustup_proxy(&f));

    // `rustup which` answers are re-asked when the toolchain binary behind them changes.
    let bin = cache.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    let fake_rustc = bin.join("fake-rustc");
    fs::write(&fake_rustc, b"old").unwrap();
    let log = cache.path().join("which.log");
    write_script(
        &bin.join("rustup"),
        &format!(
            "#!/bin/sh\necho \"$*\" >> '{}'\necho '{}'\n",
            log.display(),
            fake_rustc.display()
        ),
    );
    assert_eq!(rustup_which_rustc(&bin.join("rustup")).unwrap(), fake_rustc);
    assert_eq!(rustup_which_rustc(&bin.join("rustup")).unwrap(), fake_rustc);
    assert_eq!(count_lines(&log, |l| l == "which rustc"), 1);
    bump_mtime(&fake_rustc);
    assert_eq!(rustup_which_rustc(&bin.join("rustup")).unwrap(), fake_rustc);
    assert_eq!(
        count_lines(&log, |l| l == "which rustc"),
        2,
        "a replaced toolchain re-resolves"
    );
    assert_eq!(rustup_which_rustc(&bin.join("rustup")).unwrap(), fake_rustc);
    assert_eq!(
        count_lines(&log, |l| l == "which rustc"),
        2,
        "and the fresh answer is cached"
    );
}
