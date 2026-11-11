/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Which Rust toolchain a build uses and why. `mitts +nightly build` selects a rustup
//! toolchain like `cargo +nightly build`: the first argument is removed and becomes
//! `RUSTUP_TOOLCHAIN` for the whole command, which every rustc, rustdoc and
//! clippy-driver lookup already honours and which is part of every action key.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

static FROM_COMMAND_LINE: OnceLock<String> = OnceLock::new();

/// Splits a leading `+toolchain` off the arguments. Returns the toolchain and the
/// arguments without it; `Err` for a bare `+`.
pub fn split_toolchain_arg(args: Vec<OsString>) -> Result<(Option<String>, Vec<OsString>), String> {
    let Some(name) = args
        .get(1)
        .and_then(|a| a.to_str())
        .and_then(|a| a.strip_prefix('+'))
        .map(str::to_string)
    else {
        return Ok((None, args));
    };
    if name.is_empty() {
        return Err(
            "`+` must be followed by a toolchain name, as in `mitts +nightly build`".into(),
        );
    }
    let mut rest = args;
    rest.remove(1);
    Ok((Some(name), rest))
}

/// Selects `toolchain` for this process after checking that rustup knows it.
pub fn select(toolchain: &str) -> Result<(), String> {
    let rustup = rustup_binary().ok_or_else(|| {
        format!("`+{toolchain}` selects a rustup toolchain, but rustup was not found on PATH or in ~/.cargo/bin")
    })?;
    let output = std::process::Command::new(&rustup)
        .args(["which", "rustc", "--toolchain", toolchain])
        .output()
        .map_err(|e| format!("cannot run {}: {e}", rustup.display()))?;
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "toolchain `{toolchain}` is not usable: {}",
            message.trim().trim_start_matches("error: ")
        ));
    }
    std::env::set_var("RUSTUP_TOOLCHAIN", toolchain);
    let _ = FROM_COMMAND_LINE.set(toolchain.to_string());
    Ok(())
}

fn rustup_binary() -> Option<PathBuf> {
    let on_path = std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join("rustup"))
            .find(|p| p.is_file())
    });
    on_path.or_else(|| {
        std::env::var_os("HOME")
            .map(|home| PathBuf::from(home).join(".cargo/bin/rustup"))
            .filter(|p| p.is_file())
    })
}

/// The rustup toolchain directory name of a compiler path (`.../toolchains/<name>/bin/rustc`).
pub fn toolchain_name(rustc: &Path) -> Option<String> {
    let mut parts = rustc.iter();
    while let Some(part) = parts.next() {
        if part == "toolchains" {
            return parts.next().map(|n| n.to_string_lossy().into_owned());
        }
    }
    None
}

/// Where the choice of toolchain came from, in the order rustup applies them.
/// `RUSTC` comes first because it wins over every rustup choice, `+toolchain` included,
/// as it does for Cargo.
pub fn selection_source(start_dir: &Path) -> String {
    if std::env::var_os("RUSTC").is_some_and(|v| !v.is_empty()) {
        return "RUSTC".to_string();
    }
    if let Some(name) = FROM_COMMAND_LINE.get() {
        return format!("+{name} on the command line");
    }
    if std::env::var_os("RUSTUP_TOOLCHAIN").is_some_and(|v| !v.is_empty()) {
        return "RUSTUP_TOOLCHAIN".to_string();
    }
    for dir in start_dir.ancestors() {
        for file in ["rust-toolchain.toml", "rust-toolchain"] {
            let path = dir.join(file);
            if path.is_file() {
                return path.display().to_string();
            }
        }
    }
    "rustup default or directory override".to_string()
}

/// `toolchain: rustc 1.99.1 (58cf8f9c3 2026-10-13) · stable-aarch64-unknown-linux-gnu`,
/// with the compiler path and the source of the choice under `-v`.
pub fn toolchain_line(
    rustc_version: &str,
    rustc_path: &Path,
    verbose: bool,
    start_dir: &Path,
) -> String {
    let mut line = format!("toolchain: {rustc_version}");
    if let Some(name) = toolchain_name(rustc_path) {
        line.push_str(&format!(" · {name}"));
    }
    if verbose {
        line.push_str(&format!(
            " · {} (selected by {})",
            rustc_path.display(),
            selection_source(start_dir)
        ));
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    #[test]
    fn a_leading_plus_argument_is_split_off() {
        let (tc, rest) = split_toolchain_arg(os(&["mitts", "+nightly", "build", "-r"])).unwrap();
        assert_eq!(tc.as_deref(), Some("nightly"));
        assert_eq!(rest, os(&["mitts", "build", "-r"]));
        let (tc, rest) = split_toolchain_arg(os(&["mitts", "build", "+x"])).unwrap();
        assert_eq!(tc, None);
        assert_eq!(rest, os(&["mitts", "build", "+x"]));
        assert!(split_toolchain_arg(os(&["mitts", "+", "build"])).is_err());
    }

    #[test]
    fn toolchain_names_come_from_rustup_paths() {
        assert_eq!(
            toolchain_name(Path::new(
                "/h/.rustup/toolchains/nightly-x86_64-unknown-linux-gnu/bin/rustc"
            ))
            .as_deref(),
            Some("nightly-x86_64-unknown-linux-gnu")
        );
        assert_eq!(toolchain_name(Path::new("/usr/bin/rustc")), None);
        let line = toolchain_line(
            "rustc 1.99.1 (58cf8f9c3 2026-10-13)",
            Path::new("/h/.rustup/toolchains/stable-a/bin/rustc"),
            false,
            Path::new("/"),
        );
        assert_eq!(
            line,
            "toolchain: rustc 1.99.1 (58cf8f9c3 2026-10-13) · stable-a"
        );
    }
}
