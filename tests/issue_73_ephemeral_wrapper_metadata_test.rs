/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Regression test for issue #73: non-leaf crates under ephemeral staging (`RUSTC_WRAPPER` without
//! cache opt-in, or `--no-cache`) must receive deterministic `-C metadata` and `-C extra-filename`
//! values across invocations rather than per-process `:unhashable:<nonce>` hashes.

#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;

fn write_file(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

fn write_executable(path: &Path, body: &str) {
    write_file(path, body);
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

/// Parses `(crate_name -> (metadata, extra_filename))` from logged wrapper lines.
fn parse_crate_codegen_ids(lines: &[String]) -> BTreeMap<String, (String, String)> {
    let mut map = BTreeMap::new();
    for line in lines {
        let tokens: Vec<&str> = line.split_whitespace().collect();
        let mut crate_name = None;
        let mut metadata = None;
        let mut extra_filename = None;
        let mut i = 0;
        while i < tokens.len() {
            match tokens[i] {
                "--crate-name" if i + 1 < tokens.len() => {
                    crate_name = Some(tokens[i + 1].to_string());
                    i += 2;
                }
                "-C" if i + 1 < tokens.len() => {
                    if let Some(m) = tokens[i + 1].strip_prefix("metadata=") {
                        metadata = Some(m.to_string());
                    } else if let Some(ef) = tokens[i + 1].strip_prefix("extra-filename=") {
                        extra_filename = Some(ef.to_string());
                    }
                    i += 2;
                }
                tok => {
                    if let Some(m) = tok.strip_prefix("-Cmetadata=") {
                        metadata = Some(m.to_string());
                    } else if let Some(ef) = tok.strip_prefix("-Cextra-filename=") {
                        extra_filename = Some(ef.to_string());
                    }
                    i += 1;
                }
            }
        }
        if let Some(name) = crate_name.filter(|n| n != "build_script_build") {
            map.insert(
                name,
                (
                    metadata.expect("expected -C metadata"),
                    extra_filename.expect("expected -C extra-filename"),
                ),
            );
        }
    }
    map
}

fn run_mitts(root: &Path, cache: &Path, target: &Path, home: &Path, wrapper: &Path) -> Output {
    let out = Command::new(env!("CARGO_BIN_EXE_mitts"))
        .current_dir(root)
        .args(["check", "--workspace", "--offline", "--cache-dir"])
        .arg(cache)
        .arg("--target-dir")
        .arg(target)
        .env("CARGO_HOME", home)
        .env("RUSTC_WRAPPER", wrapper)
        .env_remove("CARGO_BUILD_RUSTC_WRAPPER")
        .env_remove("RUSTC_WORKSPACE_WRAPPER")
        .env_remove("CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER")
        .env_remove("MITTS_RUSTC_WRAPPER_CACHE")
        .env_remove("MITTS_RUSTC_WRAPPER_SALT")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

fn take_lines(log: &Path) -> Vec<String> {
    let content = fs::read_to_string(log).unwrap_or_default();
    let _ = fs::remove_file(log);
    content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(str::to_string)
        .collect()
}

#[test]
fn issue_73_ephemeral_wrapper_runs_keep_stable_metadata_and_extra_filename() {
    let dir = TempDir::new().unwrap();
    let root: PathBuf = dir.path().join("ws");
    let cache = dir.path().join("cache");
    let target = dir.path().join("target");
    let home = dir.path().join("cargo-home");
    let wrapper = dir.path().join("wrap.sh");
    let log = dir.path().join("wrapper.log");

    // Repro `a -> b -> c` workspace: `a` is leaf, `b` depends on `a` and has a build.rs writing to
    // OUT_DIR (`:bd:` + `:out:` composite suffix), and `c` depends on `b`.
    write_file(
        &root.join("Cargo.toml"),
        "[workspace]\nresolver = \"2\"\nmembers = [\"crates/a\", \"crates/b\", \"crates/c\"]\n",
    );
    write_file(
        &root.join("crates/a/Cargo.toml"),
        "[package]\nname = \"a\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write_file(
        &root.join("crates/a/src/lib.rs"),
        "pub fn f() -> u32 { 1 }\n",
    );
    write_file(
        &root.join("crates/b/Cargo.toml"),
        "[package]\nname = \"b\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
         [dependencies]\na = { path = \"../a\" }\n",
    );
    write_file(
        &root.join("crates/b/build.rs"),
        "fn main() {\n\
             let out = std::env::var(\"OUT_DIR\").unwrap();\n\
             std::fs::write(std::path::Path::new(&out).join(\"gen.rs\"), \"pub const K: u32 = 7;\").unwrap();\n\
         }\n",
    );
    write_file(
        &root.join("crates/b/src/lib.rs"),
        "include!(concat!(env!(\"OUT_DIR\"), \"/gen.rs\"));\npub fn g() -> u32 { a::f() + K }\n",
    );
    write_file(
        &root.join("crates/c/Cargo.toml"),
        "[package]\nname = \"c\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
         [dependencies]\nb = { path = \"../b\" }\n",
    );
    write_file(
        &root.join("crates/c/src/lib.rs"),
        "pub fn h() -> u32 { b::g() + 1 }\n",
    );

    write_executable(
        &wrapper,
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nexec \"$@\"\n",
            log.display()
        ),
    );

    run_mitts(&root, &cache, &target, &home, &wrapper);
    let ids1 = parse_crate_codegen_ids(&take_lines(&log));
    assert_eq!(
        ids1.keys().cloned().collect::<Vec<_>>(),
        vec!["a", "b", "c"]
    );

    let (_, b_extra_1) = &ids1["b"];
    let b_rmeta_path = target
        .join("debug/deps")
        .join(format!("libb{b_extra_1}.rmeta"));
    let b_rmeta_bytes_1 = fs::read(&b_rmeta_path).expect("libb rmeta after run 1");

    run_mitts(&root, &cache, &target, &home, &wrapper);
    let ids2 = parse_crate_codegen_ids(&take_lines(&log));

    assert_eq!(
        ids1, ids2,
        "every unit (leaf and non-leaf) must keep identical -C metadata and -C extra-filename across ephemeral runs"
    );
    for (unit, (meta, extra)) in &ids2 {
        assert_eq!(meta.len(), 16, "{unit} metadata length: {meta}");
        assert!(
            meta.bytes().all(|b| b.is_ascii_hexdigit()),
            "{unit}: {meta}"
        );
        assert_eq!(extra, &format!("-{meta}"), "{unit} extra-filename");
    }

    let b_rmeta_bytes_2 = fs::read(&b_rmeta_path).expect("libb rmeta after run 2");
    assert_eq!(
        b_rmeta_bytes_1, b_rmeta_bytes_2,
        "non-leaf libb .rmeta must be byte-identical across unchanged ephemeral runs"
    );
}
