/*
 * Mitts Build — one Cargo configuration value, with Cargo's precedence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Reads a single dotted key (`build.jobs`, `net.offline`, `target.<triple>.runner`) the
//! way Cargo does: a `--config` value wins, then the `CARGO_<KEY>` environment variable
//! (`build.rustc-wrapper` → `CARGO_BUILD_RUSTC_WRAPPER`), then the nearest
//! `.cargo/config.toml` (closer directories over their ancestors over `$CARGO_HOME`).
//! A value that names a program or a path remembers where it came from, so a relative
//! one resolves like Cargo's: against the directory holding the `.cargo` directory of its
//! file, or the current directory for an environment variable or an inline `--config`.

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

/// A configuration value and the directory its relative paths start from.
#[derive(Debug, Clone, PartialEq)]
pub struct ConfigValue {
    pub value: toml::Value,
    pub base: PathBuf,
    /// Where it came from, for error messages: a file path or `environment variable X`.
    pub origin: String,
}

/// `CARGO_` + the key in upper case with `.` and `-` as `_`.
pub fn env_var_name(key: &str) -> String {
    format!(
        "CARGO_{}",
        key.replace(['.', '-'], "_").to_ascii_uppercase()
    )
}

fn lookup<'a>(mut value: &'a toml::Value, key: &str) -> Option<&'a toml::Value> {
    for part in split_key(key) {
        value = value.get(part.as_str())?;
    }
    Some(value)
}

/// Splits a dotted key; a part in single quotes may hold dots (`target.'cfg(unix)'.runner`).
fn split_key(key: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for c in key.chars() {
        match c {
            '\'' => quoted = !quoted,
            '.' if !quoted => parts.push(std::mem::take(&mut current)),
            _ => current.push(c),
        }
    }
    parts.push(current);
    parts
}

fn read_config(path: &Path) -> Result<toml::Value> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("cannot read cargo config {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("cannot parse cargo config {}", path.display()))
}

fn cwd() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// The value of `key` for a command started in `start_dir`, or `None` when nothing sets it.
pub fn get(start_dir: &Path, key: &str) -> Result<Option<ConfigValue>> {
    let files = super::cargo_config::discover_cargo_config_files(start_dir);
    let (cli, discovered): (Vec<PathBuf>, Vec<PathBuf>) = files
        .into_iter()
        .partition(|path| super::cargo_config_cli::position(path).is_some());
    for path in cli.iter().rev() {
        if let Some(value) = lookup(&read_config(path)?, key) {
            return Ok(Some(ConfigValue {
                value: value.clone(),
                base: super::cargo_config_cli::base_dir(path),
                origin: format!("--config {}", path.display()),
            }));
        }
    }
    // A quoted part (`target.'cfg(unix)'.runner`) has no environment variable.
    let var = env_var_name(key);
    if let Some(raw) = std::env::var_os(&var).filter(|_| !key.contains('\'')) {
        let Some(raw) = raw.to_str() else {
            bail!("{var} is not valid UTF-8");
        };
        return Ok(Some(ConfigValue {
            value: toml::Value::String(raw.to_string()),
            base: cwd(),
            origin: format!("environment variable {var}"),
        }));
    }
    for path in discovered.iter().rev() {
        if let Some(value) = lookup(&read_config(path)?, key) {
            return Ok(Some(ConfigValue {
                value: value.clone(),
                base: super::cargo_config_cli::base_dir(path),
                origin: path.display().to_string(),
            }));
        }
    }
    Ok(None)
}

/// Every table found under `key` across all files, lowest precedence first (no environment
/// variables: tables such as `[alias]` or `target.<triple>.<links>` have none of their own).
pub fn tables(start_dir: &Path, key: &str) -> Result<Vec<(toml::Table, PathBuf, String)>> {
    let mut out = Vec::new();
    let files = super::cargo_config::discover_cargo_config_files(start_dir);
    let (cli, discovered): (Vec<PathBuf>, Vec<PathBuf>) = files
        .into_iter()
        .partition(|path| super::cargo_config_cli::position(path).is_some());
    for path in discovered.iter().chain(cli.iter()) {
        if let Some(table) = lookup(&read_config(path)?, key).and_then(|v| v.as_table()) {
            out.push((
                table.clone(),
                super::cargo_config_cli::base_dir(path),
                path.display().to_string(),
            ));
        }
    }
    Ok(out)
}

impl ConfigValue {
    pub fn as_bool(&self) -> Result<bool> {
        match &self.value {
            toml::Value::Boolean(b) => Ok(*b),
            toml::Value::String(s) if s == "true" => Ok(true),
            toml::Value::String(s) if s == "false" => Ok(false),
            other => bail!("{}: expected a boolean, found `{other}`", self.origin),
        }
    }

    pub fn as_i64(&self) -> Result<i64> {
        match &self.value {
            toml::Value::Integer(n) => Ok(*n),
            toml::Value::String(s) => s
                .trim()
                .parse()
                .with_context(|| format!("{}: expected an integer, found `{s}`", self.origin)),
            other => bail!("{}: expected an integer, found `{other}`", self.origin),
        }
    }

    pub fn as_str(&self) -> Result<&str> {
        match &self.value {
            toml::Value::String(s) => Ok(s),
            other => bail!("{}: expected a string, found `{other}`", self.origin),
        }
    }

    /// A list of strings: a TOML array, or a string split on whitespace (as Cargo does for
    /// `rustdocflags`, `runner` and aliases written as one string).
    pub fn as_string_list(&self) -> Result<Vec<String>> {
        match &self.value {
            toml::Value::String(s) => Ok(s.split_whitespace().map(str::to_string).collect()),
            toml::Value::Array(items) => items
                .iter()
                .map(|item| match item {
                    toml::Value::String(s) => Ok(s.clone()),
                    other => bail!("{}: expected strings, found `{other}`", self.origin),
                })
                .collect(),
            other => bail!(
                "{}: expected a string or an array, found `{other}`",
                self.origin
            ),
        }
    }

    /// A path value: relative to [`Self::base`].
    pub fn as_path(&self) -> Result<PathBuf> {
        Ok(self.base.join(self.as_str()?))
    }

    /// A program: a bare name is looked up on `PATH` when it runs; a name with a path
    /// separator is relative to [`Self::base`] (Cargo's `ConfigRelativePath` for programs).
    pub fn resolve_program(base: &Path, program: &str) -> PathBuf {
        if program.contains(std::path::MAIN_SEPARATOR) || program.contains('/') {
            base.join(program)
        } else {
            PathBuf::from(program)
        }
    }
}

#[cfg(test)]
#[path = "cargo_config_value_tests.rs"]
mod tests;
