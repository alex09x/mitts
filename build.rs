/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::process::Command;

/// The commit a crates.io package was cut from: `cargo package` writes it into
/// `.cargo_vcs_info.json` at the package root, where no `.git` exists.
fn packaged_sha() -> Option<String> {
    let dir = std::env::var_os("CARGO_MANIFEST_DIR")?;
    let text = std::fs::read_to_string(std::path::Path::new(&dir).join(".cargo_vcs_info.json")).ok()?;
    let key = "\"sha1\"";
    let rest = &text[text.find(key)? + key.len()..];
    let start = rest.find('"')? + 1;
    let end = start + rest[start..].find('"')?;
    let sha = rest[start..end].trim();
    (sha.len() >= 7 && sha.chars().all(|c| c.is_ascii_hexdigit())).then(|| sha.to_string())
}

fn main() {
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");
    println!("cargo:rerun-if-changed=.cargo_vcs_info.json");
    println!("cargo:rerun-if-changed=src");

    let in_git = std::path::Path::new(".git").exists();
    let sha = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .and_then(|out| {
            if in_git && out.status.success() {
                String::from_utf8(out.stdout)
                    .ok()
                    .map(|s| s.trim().to_string())
            } else {
                None
            }
        })
        .or_else(packaged_sha)
        .unwrap_or_else(|| "unknown".to_string());

    let is_dirty = in_git
        && Command::new("git")
            .args(["status", "--porcelain", "--untracked-files=no"])
            .output()
            .ok()
            .map(|out| out.status.success() && !out.stdout.is_empty())
            .unwrap_or(false);

    let version_suffix = if is_dirty {
        format!("{}-dirty", sha)
    } else {
        sha
    };

    println!("cargo:rustc-env=MITTS_GIT_SHA={}", version_suffix);
}
