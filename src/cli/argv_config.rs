/*
 * Mitts Build — command-line changes that come from Cargo configuration
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Before the command line is parsed, as Cargo does:
//!
//! - `--config` values on it are installed, so everything below sees them;
//! - an unknown subcommand is looked up in `[alias]` (or `CARGO_ALIAS_<NAME>`), then in
//!   Cargo's built-in `b`, `c`, `t`, `r`; aliases may name other aliases, never themselves;
//! - `net.offline = true` adds `--offline`, `term.quiet` / `term.verbose` add `--quiet` /
//!   `--verbose`, each only to a command that has that option and only when the command
//!   line does not already say it.
//!
//! A user alias never shadows a built-in command (Cargo warns and ignores it too).

use crate::adapter::rust::{cargo_config_cli, cargo_config_tools, cargo_config_value};
use anyhow::{bail, Result};
use clap::CommandFactory;
use std::ffi::OsString;

const BUILTIN_ALIASES: [(&str, &str); 4] =
    [("b", "build"), ("c", "check"), ("t", "test"), ("r", "run")];
/// Global options that take their value as the next word.
const GLOBAL_WITH_VALUE: [&str; 2] = ["--color", "-Z"];

/// The index of the subcommand word, after `argv[0]` and global options.
fn subcommand_index(args: &[OsString]) -> Option<usize> {
    let mut i = 1;
    while i < args.len() {
        let word = args[i].to_str()?;
        if word == "--" {
            return None;
        }
        if !word.starts_with('-') {
            return Some(i);
        }
        i += if GLOBAL_WITH_VALUE.contains(&word) {
            2
        } else {
            1
        };
    }
    None
}

/// `--config X` / `--config=X` values after the subcommand, up to a `--`.
fn config_values(args: &[OsString], from: usize) -> Vec<String> {
    let mut values = Vec::new();
    let mut i = from;
    while i < args.len() {
        let Some(word) = args[i].to_str() else {
            i += 1;
            continue;
        };
        if word == "--" {
            break;
        }
        if word == "--config" {
            if let Some(value) = args.get(i + 1).and_then(|v| v.to_str()) {
                values.push(value.to_string());
            }
            i += 2;
            continue;
        }
        if let Some(value) = word.strip_prefix("--config=") {
            values.push(value.to_string());
        }
        i += 1;
    }
    values
}

/// `args` without the `--config X` / `--config=X` words after `from`, up to a `--`.
fn without_config(args: Vec<OsString>, from: usize) -> Vec<OsString> {
    let mut out = Vec::with_capacity(args.len());
    let mut i = 0;
    let mut passthrough = false;
    while i < args.len() {
        let word = args[i].to_str().unwrap_or("");
        if i >= from && !passthrough {
            if word == "--" {
                passthrough = true;
            } else if word == "--config" {
                i += 2;
                continue;
            } else if word.starts_with("--config=") {
                i += 1;
                continue;
            }
        }
        out.push(args[i].clone());
        i += 1;
    }
    out
}

fn is_builtin(name: &str) -> bool {
    crate::cli::Cli::command().find_subcommand(name).is_some()
}

/// Expands `args[at]` while it names an alias.
fn expand_aliases(args: &mut Vec<OsString>, at: usize) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let mut seen: Vec<String> = Vec::new();
    loop {
        let Some(name) = args[at].to_str().map(str::to_string) else {
            return Ok(());
        };
        if is_builtin(&name) {
            return Ok(());
        }
        if seen.contains(&name) {
            bail!(
                "alias `{name}` has an unresolvable recursive definition: {}",
                seen.join(" -> ")
            );
        }
        let expansion = match cargo_config_value::get(&cwd, &format!("alias.{name}"))? {
            Some(value) => value.as_string_list()?,
            None => match BUILTIN_ALIASES.iter().find(|(alias, _)| *alias == name) {
                Some((_, command)) => vec![command.to_string()],
                None => return Ok(()),
            },
        };
        if expansion.is_empty() {
            bail!("alias `{name}` is empty");
        }
        seen.push(name);
        args.splice(at..=at, expansion.into_iter().map(OsString::from));
    }
}

/// Whether one of `flags` is on the command line, also inside a bundle of short flags
/// (`-vv`, `-vq`) for the single-letter ones.
fn has_flag(args: &[OsString], from: usize, flags: &[&str]) -> bool {
    let letters: Vec<char> = flags
        .iter()
        .filter_map(|f| f.strip_prefix('-').filter(|s| s.len() == 1))
        .filter_map(|s| s.chars().next())
        .collect();
    args[from..]
        .iter()
        .take_while(|word| *word != "--")
        .filter_map(|word| word.to_str())
        .any(|word| {
            flags.contains(&word)
                || (word.len() > 2
                    && word.starts_with('-')
                    && !word.starts_with("--")
                    && word[1..].chars().all(|c| c.is_ascii_alphabetic())
                    && word[1..].chars().any(|c| letters.contains(&c)))
        })
}

/// `-C DIR` (or `-CDIR`) before the subcommand, as in Cargo: the process moves to DIR
/// and the option is dropped, so configuration and manifest discovery start there.
pub fn change_directory(mut args: Vec<OsString>) -> Result<Vec<OsString>> {
    let mut i = 1;
    while i < args.len() {
        let Some(word) = args[i].to_str().map(str::to_string) else {
            return Ok(args);
        };
        if word == "--" || !word.starts_with('-') {
            return Ok(args);
        }
        let dir = if word == "-C" {
            let Some(dir) = args.get(i + 1).cloned() else {
                bail!("a value is required for '-C <DIRECTORY>' but none was supplied");
            };
            args.drain(i..i + 2);
            dir
        } else if let Some(dir) = word.strip_prefix("-C").filter(|d| !d.is_empty()) {
            let dir = OsString::from(dir);
            args.remove(i);
            dir
        } else {
            i += if GLOBAL_WITH_VALUE.contains(&word.as_str()) {
                2
            } else {
                1
            };
            continue;
        };
        std::env::set_current_dir(&dir).map_err(|e| {
            anyhow::anyhow!(
                "failed to change working directory to `{}`: {e}",
                std::path::Path::new(&dir).display()
            )
        })?;
        return Ok(args);
    }
    Ok(args)
}

/// Rewrites `args` (with `argv[0]`, without a `+toolchain`) as the configuration says.
pub fn apply(mut args: Vec<OsString>) -> Result<Vec<OsString>> {
    let Some(at) = subcommand_index(&args) else {
        return Ok(args);
    };
    cargo_config_cli::set(&config_values(&args, at + 1))?;
    expand_aliases(&mut args, at)?;
    let Some(name) = args[at].to_str().map(str::to_string) else {
        return Ok(args);
    };
    let Some(command) = crate::cli::Cli::command().find_subcommand(&name).cloned() else {
        return Ok(args);
    };
    // An alias may have added `--config` values of its own.
    cargo_config_cli::set(&config_values(&args, at + 1))?;
    let has_option = |id: &str| command.get_arguments().any(|arg| arg.get_id() == id);
    // Every Cargo command takes `--config`; the values are installed above, so a command
    // without its own option just drops them.
    if !has_option("config") {
        args = without_config(args, at + 1);
    }
    let cwd = std::env::current_dir()?;
    let mut inserted: Vec<OsString> = Vec::new();
    if cargo_config_tools::offline()
        && has_option("offline")
        && !has_flag(&args, at + 1, &["--offline"])
    {
        inserted.push("--offline".into());
    }
    let quiet = cargo_config_value::get(&cwd, "term.quiet")?
        .map(|v| v.as_bool())
        .transpose()?
        .unwrap_or(false);
    let verbose = cargo_config_value::get(&cwd, "term.verbose")?
        .map(|v| v.as_bool())
        .transpose()?
        .unwrap_or(false);
    if quiet && verbose {
        bail!("cannot set both `term.verbose` and `term.quiet`");
    }
    if !has_flag(&args, at + 1, &["-q", "--quiet", "-v", "--verbose"]) {
        if quiet && has_option("quiet") {
            inserted.push("--quiet".into());
        }
        if verbose && has_option("verbose") {
            inserted.push("--verbose".into());
        }
    }
    args.splice(at + 1..at + 1, inserted);
    Ok(args)
}

#[cfg(test)]
#[path = "argv_config_tests.rs"]
mod tests;
