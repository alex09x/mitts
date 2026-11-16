/*
 * Mitts Build — `target.<triple>.runner` from Cargo configuration
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! `mitts run`, `test` and `bench` start an executable through its runner when the
//! configuration names one (qemu, wine, a simulator, `sudo -E`, ...), as Cargo does:
//! `target.<triple>.runner` (or `CARGO_TARGET_<TRIPLE>_RUNNER`, or `--config`) first, else
//! the one `target.'cfg(..)'.runner` whose expression matches; two matching `cfg` entries
//! are an error. The value is a program and its arguments, as an array or one string; a
//! program with a path separator is relative to its config file's directory.

use super::cargo_config_value::{self, ConfigValue};
use anyhow::{bail, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The runner and its arguments for executables built for `target`, if one is set.
pub fn runner(start_dir: &Path, target: &str) -> Result<Option<Vec<String>>> {
    if let Some(value) = cargo_config_value::get(start_dir, &format!("target.{target}.runner"))? {
        return to_command(&value).map(Some);
    }
    // Nearest file wins per key; then exactly one matching `cfg(..)` key may set a runner.
    let mut by_key: BTreeMap<String, ConfigValue> = BTreeMap::new();
    for (table, base, origin) in cargo_config_value::tables(start_dir, "target")? {
        for (key, entry) in table {
            if !key.starts_with("cfg(") {
                continue;
            }
            if let Some(runner) = entry.get("runner") {
                by_key.insert(
                    key,
                    ConfigValue {
                        value: runner.clone(),
                        base: base.clone(),
                        origin: origin.clone(),
                    },
                );
            }
        }
    }
    let host = super::compiler_resolve::try_resolve_host_triple()?;
    let explicit = (target != host).then_some(target);
    let mut matched = Vec::new();
    for (key, value) in by_key {
        let hit = super::target_filter::eval_base_target_cfg(&key, explicit)
            .map_err(|e| anyhow::anyhow!("target.'{key}'.runner in {}: {e}", value.origin))?;
        if hit {
            matched.push((key, value));
        }
    }
    match matched.len() {
        0 => Ok(None),
        1 => to_command(&matched[0].1).map(Some),
        _ => bail!(
            "several matching instances of `target.'cfg(..)'.runner` in configurations: {}",
            matched
                .iter()
                .map(|(key, _)| format!("target.'{key}'"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn to_command(value: &ConfigValue) -> Result<Vec<String>> {
    let mut parts = value.as_string_list()?;
    if parts.is_empty() {
        bail!("{}: runner is empty", value.origin);
    }
    parts[0] = ConfigValue::resolve_program(&value.base, &parts[0])
        .to_string_lossy()
        .into_owned();
    Ok(parts)
}

/// The triple executables are built for: `--target`, else `build.target`, else the host.
pub fn effective_target(start_dir: &Path, explicit: Option<&str>) -> Result<String> {
    if let Some(target) = explicit {
        return Ok(target.to_string());
    }
    if let Some(target) = super::cargo_config::load_cargo_config_build_target(start_dir)? {
        return Ok(target);
    }
    super::compiler_resolve::try_resolve_host_triple()
}

/// A command that runs `binary`, through the configured runner if there is one.
pub fn command(binary: &Path, start_dir: &Path, explicit_target: Option<&str>) -> Result<Command> {
    let target = effective_target(start_dir, explicit_target)?;
    Ok(match runner(start_dir, &target)? {
        Some(parts) => {
            let mut cmd = Command::new(PathBuf::from(&parts[0]));
            cmd.args(&parts[1..]).arg(binary);
            cmd
        }
        None => Command::new(binary),
    })
}

#[cfg(test)]
#[path = "cargo_config_runner_tests.rs"]
mod tests;
