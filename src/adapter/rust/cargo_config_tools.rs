/*
 * Mitts Build — command-wide settings from Cargo configuration
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! The `build.*` and `net.*` keys that hold for a whole command: `build.jobs`,
//! `build.rustc`, `build.rustdoc`, `build.incremental`, `net.offline`, `net.retry`. They are
//! read once per command from the directory it starts in (Cargo reads its configuration
//! from the current directory too), after the `--config` values are installed, and kept
//! here so that hot paths such as resolving rustc do not reread the files.

use super::cargo_config_value::{self, ConfigValue};
use anyhow::{bail, Result};
use std::path::{Path, PathBuf};
use std::sync::RwLock;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tools {
    pub rustc: Option<PathBuf>,
    pub rustdoc: Option<PathBuf>,
    pub incremental: Option<bool>,
    pub offline: bool,
    pub retry: Option<usize>,
}

static TOOLS: RwLock<Option<Tools>> = RwLock::new(None);

/// Rereads the settings for a command started in `start_dir`.
pub fn refresh(start_dir: &Path) -> Result<()> {
    let tools = read(start_dir)?;
    crate::core::jobs::set_config_limit(read_jobs(start_dir)?);
    *TOOLS.write().unwrap_or_else(|e| e.into_inner()) = Some(tools);
    Ok(())
}

fn program(value: Option<ConfigValue>) -> Result<Option<PathBuf>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let program = value.as_str()?.trim();
    if program.is_empty() {
        return Ok(None);
    }
    Ok(Some(ConfigValue::resolve_program(&value.base, program)))
}

fn read(start_dir: &Path) -> Result<Tools> {
    let get = |key: &str| cargo_config_value::get(start_dir, key);
    let retry = match get("net.retry")? {
        Some(value) => {
            let n = value.as_i64()?;
            if n < 0 {
                bail!("{}: net.retry must not be negative", value.origin);
            }
            Some(n as usize)
        }
        None => None,
    };
    Ok(Tools {
        rustc: program(get("build.rustc")?)?,
        rustdoc: program(get("build.rustdoc")?)?,
        incremental: get("build.incremental")?.map(|v| v.as_bool()).transpose()?,
        offline: get("net.offline")?
            .map(|v| v.as_bool())
            .transpose()?
            .unwrap_or(false),
        retry,
    })
}

/// `build.jobs`: a positive count, a negative one (that many CPUs fewer, at least one) or
/// `"default"`. Zero means unset.
fn read_jobs(start_dir: &Path) -> Result<usize> {
    let Some(value) = cargo_config_value::get(start_dir, "build.jobs")? else {
        return Ok(0);
    };
    if value.as_str().is_ok_and(|s| s.trim() == "default") {
        return Ok(0);
    }
    let n = value.as_i64()?;
    let cpus = crate::core::jobs::cpus() as i64;
    match n {
        0 => bail!("{}: build.jobs may not be 0", value.origin),
        n if n > 0 => Ok(n as usize),
        n => Ok((cpus + n).max(1) as usize),
    }
}

fn current() -> Tools {
    TOOLS
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .unwrap_or_default()
}

/// `build.rustc` (or `CARGO_BUILD_RUSTC`); `RUSTC` wins over it, as in Cargo.
pub fn rustc() -> Option<PathBuf> {
    current().rustc
}

/// `build.rustdoc` (or `CARGO_BUILD_RUSTDOC`); `RUSTDOC` wins over it.
pub fn rustdoc() -> Option<PathBuf> {
    current().rustdoc
}

/// `build.incremental` (or `CARGO_BUILD_INCREMENTAL`); `CARGO_INCREMENTAL` wins over it, and
/// it wins over the profile's `incremental`.
pub fn incremental() -> Option<bool> {
    current().incremental
}

/// `net.offline` (or `CARGO_NET_OFFLINE`): like `--offline` on every command.
pub fn offline() -> bool {
    current().offline
}

/// `net.retry` (or `CARGO_NET_RETRY`); `MITTS_NET_RETRY` wins over it.
pub fn retry() -> Option<usize> {
    current().retry
}

/// The flags for rustdoc, in Cargo's order: `CARGO_ENCODED_RUSTDOCFLAGS` (split on 0x1f),
/// else `RUSTDOCFLAGS`, else `target.<triple>.rustdocflags` (and its environment variable),
/// else `build.rustdocflags` (or `CARGO_BUILD_RUSTDOCFLAGS`).
pub fn rustdocflags(start_dir: &Path, target: &str) -> Result<Vec<String>> {
    if let Ok(encoded) = std::env::var("CARGO_ENCODED_RUSTDOCFLAGS") {
        return Ok(encoded
            .split('\u{1f}')
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect());
    }
    if let Ok(flags) = std::env::var("RUSTDOCFLAGS") {
        return Ok(flags.split_whitespace().map(str::to_string).collect());
    }
    for key in [
        format!("target.{target}.rustdocflags"),
        "build.rustdocflags".to_string(),
    ] {
        if let Some(value) = cargo_config_value::get(start_dir, &key)? {
            return value.as_string_list();
        }
    }
    Ok(Vec::new())
}

#[cfg(test)]
pub fn set_for_tests(tools: Option<Tools>) {
    *TOOLS.write().unwrap_or_else(|e| e.into_inner()) = tools;
}
