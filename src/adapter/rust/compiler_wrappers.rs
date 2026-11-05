/*
 * Mitts Build — compiler wrapper configuration
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */
use super::cargo_config::discover_cargo_config_files;
use anyhow::Context;
use anyhow::Result;
use std::fs;
use std::path::Path;

pub fn compiler_command(
    compiler: &Path,
    start_dir: &Path,
    workspace: bool,
) -> Result<std::process::Command> {
    let mut wrappers = compiler_wrappers(start_dir, workspace)?.into_iter();
    let mut cmd = if let Some(first) = wrappers.next() {
        let mut command = std::process::Command::new(first);
        command.args(wrappers);
        command.arg(compiler);
        command
    } else {
        std::process::Command::new(compiler)
    };
    cmd.env("RUSTC", compiler);
    Ok(cmd)
}

/// The warning printed when compiler wrappers make a build uncached: a wrapper (sccache, a
/// script) is an input Mitts cannot know, so no unit is reused and every run compiles again.
pub fn uncached_wrapper_warning(wrappers: &[std::ffi::OsString]) -> String {
    let names: Vec<String> = wrappers
        .iter()
        .map(|w| w.to_string_lossy().into_owned())
        .collect();
    format!(
        "[mitts::warn] a rustc wrapper is configured ({}): this build is not cached. Clear it to \
         use the cache: set RUSTC_WRAPPER= and CARGO_BUILD_RUSTC_WRAPPER= (empty), or remove \
         `rustc-wrapper` from the cargo config.",
        names.join(", ")
    )
}

/// Resolve compiler wrappers in Cargo's precedence order and invocation order.
pub fn compiler_wrappers(start_dir: &Path, workspace: bool) -> Result<Vec<std::ffi::OsString>> {
    let mut configured = [None::<std::ffi::OsString>, None::<std::ffi::OsString>];
    for path in discover_cargo_config_files(start_dir) {
        let content = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read cargo config at {:?}", path))?;
        let val: toml::Value = toml::from_str(&content)
            .with_context(|| format!("Failed to parse cargo config at {:?}", path))?;
        for (slot, key) in configured
            .iter_mut()
            .zip(["rustc-wrapper", "rustc-workspace-wrapper"])
        {
            if let Some(value) = val.get("build").and_then(|v| v.get(key)) {
                let value = value
                    .as_str()
                    .context("compiler wrapper must be a string")?;
                let path_value = Path::new(value);
                let value = if value.contains('/') && path_value.is_relative() {
                    path.parent()
                        .and_then(Path::parent)
                        .unwrap_or(start_dir)
                        .join(path_value)
                        .into_os_string()
                } else {
                    value.into()
                };
                *slot = Some(value);
            }
        }
    }
    let mut wrappers = Vec::new();
    for (i, (env, config_env)) in [
        ("RUSTC_WRAPPER", "CARGO_BUILD_RUSTC_WRAPPER"),
        (
            "RUSTC_WORKSPACE_WRAPPER",
            "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        if i == 1 && !workspace {
            continue;
        }
        let env_value = std::env::var_os(env).or_else(|| std::env::var_os(config_env));
        let value = env_value
            .map(|value| {
                let path = Path::new(&value);
                if path.is_relative() && value.to_string_lossy().contains('/') {
                    std::env::current_dir()
                        .unwrap_or_else(|_| start_dir.to_path_buf())
                        .join(path)
                        .into_os_string()
                } else {
                    value
                }
            })
            .or_else(|| configured[i].clone());
        if let Some(value) = value.filter(|v| !v.is_empty()) {
            wrappers.push(value);
        }
    }
    Ok(wrappers)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    #[test]
    fn the_warning_names_each_wrapper_and_how_to_clear_it() {
        let text = uncached_wrapper_warning(&[
            OsString::from("/home/u/.cargo/bin/sccache"),
            OsString::from("ws-wrap"),
        ]);
        assert!(text.starts_with("[mitts::warn] "));
        assert!(text.contains("/home/u/.cargo/bin/sccache, ws-wrap"));
        assert!(text.contains("not cached"));
        assert!(text.contains("RUSTC_WRAPPER="));
        assert!(text.contains("CARGO_BUILD_RUSTC_WRAPPER="));
    }
}
