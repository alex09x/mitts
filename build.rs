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
    let text =
        std::fs::read_to_string(std::path::Path::new(&dir).join(".cargo_vcs_info.json")).ok()?;
    let key = "\"sha1\"";
    let rest = &text[text.find(key)? + key.len()..];
    let start = rest.find('"')? + 1;
    let end = start + rest[start..].find('"')?;
    let sha = rest[start..end].trim();
    (sha.len() >= 7 && sha.chars().all(|c| c.is_ascii_hexdigit())).then(|| sha.to_string())
}

fn git_path(name: &str) -> std::path::PathBuf {
    Command::new("git")
        .args(["rev-parse", "--git-path", name])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|path| std::path::PathBuf::from(path.trim()))
        .unwrap_or_else(|| std::path::PathBuf::from(".git").join(name))
}

fn main() {
    let in_git = std::path::Path::new(".git").exists();
    let packaged = packaged_sha().is_some();
    // Missing watched files force Cargo to rebuild on every invocation. Keep
    // that conservative behavior only when neither provenance source is known.
    if in_git || !packaged {
        for name in ["HEAD", "index"] {
            println!("cargo:rerun-if-changed={}", git_path(name).display());
        }
        if let Ok(output) = Command::new("git")
            .args(["symbolic-ref", "-q", "HEAD"])
            .output()
        {
            if output.status.success() {
                if let Ok(reference) = String::from_utf8(output.stdout) {
                    let path = git_path(reference.trim());
                    let watched = if path.exists() {
                        path.as_path()
                    } else {
                        path.parent().unwrap_or(&path)
                    };
                    println!("cargo:rerun-if-changed={}", watched.display());
                    let packed = git_path("packed-refs");
                    if packed.exists() {
                        println!("cargo:rerun-if-changed={}", packed.display());
                    }
                }
            }
        }
    }
    if packaged || !in_git {
        println!("cargo:rerun-if-changed=.cargo_vcs_info.json");
    }
    // The dirty suffix depends on every tracked file, including docs/config.
    if in_git {
        if let Ok(output) = Command::new("git").args(["ls-files", "-z"]).output() {
            if output.status.success() {
                for path in String::from_utf8_lossy(&output.stdout)
                    .split('\0')
                    .filter(|path| !path.is_empty())
                {
                    println!("cargo:rerun-if-changed={path}");
                }
            }
        }
    }
    println!("cargo:rerun-if-changed=src");

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
            .args([
                "--no-optional-locks",
                "status",
                "--porcelain",
                "--untracked-files=no",
            ])
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
