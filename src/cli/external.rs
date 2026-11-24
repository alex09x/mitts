/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! External subcommands and `--list`, as Cargo has them: `mitts NAME ARGS` runs the
//! executable `cargo-NAME` found in `$CARGO_HOME/bin` or on `PATH` as `cargo-NAME NAME ARGS`,
//! with `CARGO` set to this Mitts, so a tool that calls `$CARGO` back goes through Mitts.

use clap::CommandFactory;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

const PREFIX: &str = "cargo-";

/// The directories searched for `cargo-NAME`, in order: `$CARGO_HOME/bin`, then `PATH`.
fn search_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = crate::core::source_store::SourceStore::active_cargo_home() {
        dirs.push(home.join("bin"));
    }
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    dirs
}

fn is_executable(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn executable_name(name: &str) -> String {
    format!("{PREFIX}{name}{}", std::env::consts::EXE_SUFFIX)
}

/// The first `cargo-NAME` in [`search_dirs`]. A name with a path separator or `..` is a
/// path, not a command name, and is never looked up.
pub fn find(name: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    if name.is_empty() || name.contains(['/', '\\']) || name.contains("..") {
        return None;
    }
    let file = executable_name(name);
    dirs.iter()
        .map(|dir| dir.join(&file))
        .find(|candidate| is_executable(candidate))
}

/// Every `cargo-NAME` executable in `dirs`, by name; the first directory wins.
pub fn installed(dirs: &[PathBuf]) -> BTreeMap<String, PathBuf> {
    let mut found = BTreeMap::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let file = entry.file_name();
            let Some(file) = file.to_str() else {
                continue;
            };
            let Some(name) = file
                .strip_prefix(PREFIX)
                .map(|n| n.strip_suffix(std::env::consts::EXE_SUFFIX).unwrap_or(n))
            else {
                continue;
            };
            if name.is_empty() || !is_executable(&entry.path()) {
                continue;
            }
            found
                .entry(name.to_string())
                .or_insert_with(|| entry.path());
        }
    }
    found
}

/// Runs `cargo-NAME NAME ARGS` (`args[0]` is NAME) and returns its exit code.
pub fn run(args: &[OsString]) -> i32 {
    let Some(name) = args.first().and_then(|n| n.to_str()) else {
        eprintln!("error: no such command");
        return 101;
    };
    let dirs = search_dirs();
    let Some(program) = find(name, &dirs) else {
        let mut message = format!("error: no such command: `{name}`");
        if let Some(close) = closest_builtin(name) {
            message.push_str(&format!("\n\n\tDid you mean `{close}`?"));
        }
        message.push_str("\n\n\tView all installed commands with `mitts --list`");
        eprintln!("{message}");
        return 101;
    };
    let mut command = std::process::Command::new(&program);
    command.args(args);
    if let Ok(exe) = std::env::current_exe() {
        command.env("CARGO", exe);
    }
    match command.status() {
        Ok(status) => status.code().unwrap_or(101),
        Err(error) => {
            eprintln!("error: failed to run `{}`: {error}", program.display());
            101
        }
    }
}

/// A built-in command within edit distance 3 of `name`, as Cargo suggests one.
fn closest_builtin(name: &str) -> Option<String> {
    crate::cli::Cli::command()
        .get_subcommands()
        .map(|c| c.get_name().to_string())
        .map(|c| (edit_distance(name, &c), c))
        .filter(|(d, _)| *d <= 3 && *d < name.len().max(1))
        .min_by_key(|(d, _)| *d)
        .map(|(_, c)| c)
}

fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for j in 0..b.len() {
            let cur = row[j + 1];
            row[j + 1] = if ca == b[j] {
                prev
            } else {
                1 + prev.min(row[j]).min(row[j + 1])
            };
            prev = cur;
        }
    }
    row[b.len()]
}

/// Whether the command line asks for `--list`: among the global flags before any subcommand
/// (`mitts --list`, `mitts -v --list`, `mitts --color never --list`).
pub fn wants_list(args: &[OsString]) -> bool {
    let mut rest = args.iter().skip(1);
    while let Some(arg) = rest.next() {
        match arg.to_str().unwrap_or("") {
            "--list" => return true,
            // Global options that take a value as the next word.
            "--color" | "--config" | "-Z" | "-C" => {
                rest.next();
            }
            a if a.starts_with('-') => {}
            _ => return false,
        }
    }
    false
}

/// `mitts --list`: built-in commands with their summary, aliases from the configuration,
/// then the external `cargo-NAME` commands found.
pub fn list() -> i32 {
    let mut lines: BTreeMap<String, String> = BTreeMap::new();
    for command in crate::cli::Cli::command().get_subcommands() {
        if command.is_hide_set() {
            continue;
        }
        let about = command
            .get_about()
            .map(|a| a.to_string())
            .unwrap_or_default();
        lines.insert(command.get_name().to_string(), about);
    }
    for (alias, target) in [("b", "build"), ("c", "check"), ("t", "test"), ("r", "run")] {
        lines.insert(alias.to_string(), format!("alias: {target}"));
    }
    if let Ok(cwd) = std::env::current_dir() {
        if let Ok(Some(config)) = crate::adapter::rust::cargo_config_value::get(&cwd, "alias") {
            for (alias, value) in config.value.as_table().into_iter().flatten() {
                let words = match value {
                    toml::Value::String(s) => s.split_whitespace().collect::<Vec<_>>().join(" "),
                    toml::Value::Array(items) => items
                        .iter()
                        .filter_map(|w| w.as_str())
                        .collect::<Vec<_>>()
                        .join(" "),
                    _ => continue,
                };
                lines.insert(alias.clone(), format!("alias: {words}"));
            }
        }
    }
    for name in installed(&search_dirs()).into_keys() {
        lines.entry(name).or_default();
    }
    println!("Installed Commands:");
    for (name, about) in lines {
        if about.is_empty() {
            println!("    {name}");
        } else {
            println!("    {name:<20} {about}");
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_with_path_parts_are_not_looked_up() {
        let dir = std::env::temp_dir();
        for name in ["x/../../tmp/tool", "a\\b", "..", ""] {
            assert_eq!(find(name, std::slice::from_ref(&dir)), None, "{name}");
        }
    }

    #[test]
    fn list_is_found_among_the_global_flags() {
        let args = |words: &[&str]| -> Vec<OsString> {
            std::iter::once("mitts")
                .chain(words.iter().copied())
                .map(OsString::from)
                .collect()
        };
        assert!(wants_list(&args(&["--list"])));
        assert!(wants_list(&args(&["-v", "--list"])));
        assert!(wants_list(&args(&["--color", "never", "--list"])));
        assert!(!wants_list(&args(&["build", "--list"])));
        assert!(!wants_list(&args(&["--color", "--list"])));
        assert!(!wants_list(&args(&[])));
    }

    #[test]
    fn suggests_a_close_builtin() {
        assert_eq!(edit_distance("biuld", "build"), 2);
        assert_eq!(closest_builtin("biuld").as_deref(), Some("build"));
        // Within 3 edits, as Cargo: an unknown `nextest` (no cargo-nextest) suggests `test`.
        assert_eq!(closest_builtin("nextest").as_deref(), Some("test"));
        assert_eq!(closest_builtin("frobnicate"), None);
    }
}
