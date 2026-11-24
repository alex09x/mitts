/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! The future-incompatibility report after every build, as Cargo gives it: a dependency
//! whose code a future Rust will reject is named at the end of `build` and `check`, from a
//! fresh compile and from the cache alike; the report is kept for
//! `mitts report future-incompatibilities`; `--future-incompat-report` prints it whole.

use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

/// `old-dep 0.1.0`, whose macro ends an expression with `;`
/// (`semicolon_in_expressions_from_macros`, reported for dependencies).
fn crate_archive() -> (Vec<u8>, String) {
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    {
        let mut tar = tar::Builder::new(&mut gz);
        let mut add = |path: &str, data: &str| {
            let mut h = tar::Header::new_gnu();
            h.set_size(data.len() as u64);
            h.set_mode(0o644);
            h.set_cksum();
            tar.append_data(&mut h, path, data.as_bytes()).unwrap();
        };
        add(
            "old-dep-0.1.0/Cargo.toml",
            "[package]\nname = \"old-dep\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        );
        add(
            "old-dep-0.1.0/src/lib.rs",
            "macro_rules! yes { () => { true; } }\npub fn flag() -> bool { let x = yes!(); x }\n",
        );
        tar.finish().unwrap();
    }
    let bytes = gz.finish().unwrap();
    let sum = format!("{:x}", Sha256::digest(&bytes));
    (bytes, sum)
}

fn project(root: &Path) {
    let (bytes, sum) = crate_archive();
    let registry = root.join("registry");
    let entry = registry.join("index/ol/d-/old-dep");
    fs::create_dir_all(entry.parent().unwrap()).unwrap();
    fs::write(
        &entry,
        format!(
            "{{\"name\":\"old-dep\",\"vers\":\"0.1.0\",\"deps\":[],\"cksum\":\"{sum}\",\"features\":{{}},\"yanked\":false}}\n"
        ),
    )
    .unwrap();
    fs::write(registry.join("old-dep-0.1.0.crate"), bytes).unwrap();
    let app = root.join("app");
    fs::create_dir_all(app.join("src")).unwrap();
    fs::create_dir_all(app.join(".cargo")).unwrap();
    fs::write(
        app.join(".cargo/config.toml"),
        format!(
            "[source.crates-io]\nreplace-with = \"local\"\n[source.local]\nlocal-registry = '{}'\n",
            registry.display()
        ),
    )
    .unwrap();
    fs::write(
        app.join("Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nold-dep = \"0.1\"\n",
    )
    .unwrap();
    fs::write(
        app.join("src/lib.rs"),
        "pub fn run() -> bool { old_dep::flag() }\n",
    )
    .unwrap();
}

fn mitts(root: &Path, args: &[&str]) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_mitts"))
        .args(args)
        .arg("--cache-dir")
        .arg(root.join("cache"))
        .current_dir(root.join("app"))
        .env("CARGO_HOME", root.join("cargo-home"))
        .env_remove("MITTS_CACHE_DIR")
        .env_remove("CARGO_TARGET_DIR")
        .env_remove("RUSTFLAGS")
        .output()
        .unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn a_dependency_with_future_incompatible_code_is_reported_after_every_build() {
    let temp = TempDir::new().unwrap();
    project(temp.path());
    let named = "the following packages contain code that will be rejected by a future version of Rust: old-dep v0.1.0";

    let (ok, err) = mitts(temp.path(), &["build", "--offline"]);
    assert!(ok, "{err}");
    assert!(err.contains(named), "{err}");
    assert!(
        err.contains("mitts report future-incompatibilities --id 1"),
        "{err}"
    );

    // The kept report names the lint.
    let out = Command::new(env!("CARGO_BIN_EXE_mitts"))
        .args(["report", "future-incompatibilities", "--id", "1"])
        .current_dir(temp.path().join("app"))
        .output()
        .unwrap();
    let report =
        String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    assert!(
        report.contains("trailing semicolon in macro used in expression position"),
        "{report}"
    );

    // From the cache: the same warning, a new report.
    let (ok, err) = mitts(temp.path(), &["build", "--offline"]);
    assert!(ok && err.contains(named), "{err}");
    assert!(err.contains("--id 2"), "{err}");

    // `check` too, and the full report on request.
    let (ok, err) = mitts(temp.path(), &["check", "--offline"]);
    assert!(ok && err.contains(named), "{err}");
    let (ok, err) = mitts(
        temp.path(),
        &["build", "--offline", "--future-incompat-report"],
    );
    assert!(ok, "{err}");
    assert!(
        err.contains("currently triggers the following future incompatibility lints"),
        "{err}"
    );

    // The dependency's own diagnostics are not shown as JSON.
    assert!(!err.contains("\"$message_type\""), "{err}");
}
