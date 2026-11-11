/*
 * Mitts Build — custom build profiles (`--profile NAME`)
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A custom profile (`[profile.ci] inherits = "release"`) is selected per Mitts run with
//! `--profile NAME`. One run builds one profile, so the selection is process state, set by
//! the CLI before the build: unit profiles resolve through its `inherits` chain, and its
//! outputs go to `target/NAME` (Cargo's layout). `dev` and `release` are not custom; they
//! keep the `release` flag and `target/debug` / `target/release`.

use super::ResolvedProfiles;
use anyhow::{anyhow, bail, Result};
use std::sync::RwLock;

static CUSTOM: RwLock<Option<String>> = RwLock::new(None);

/// Selects the custom profile of this run (`None` for `dev` / `release`).
pub fn set_custom_profile(name: Option<&str>) {
    *CUSTOM.write().unwrap_or_else(|e| e.into_inner()) = name.map(str::to_string);
}

/// The custom profile of this run, if one was selected.
pub fn custom_profile() -> Option<String> {
    CUSTOM.read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The directory under the target directory that holds this run's outputs.
pub fn profile_dir(release: bool) -> String {
    match custom_profile().as_deref() {
        // Cargo keeps the built-in test/bench profiles' outputs with dev/release.
        Some("test") => "debug".to_string(),
        Some("bench") => "release".to_string(),
        Some(custom) => custom.to_string(),
        None => if release { "release" } else { "debug" }.to_string(),
    }
}

/// The name of this run's profile, as shown to the user.
pub fn profile_name(release: bool) -> String {
    custom_profile().unwrap_or_else(|| if release { "release" } else { "dev" }.to_string())
}

/// Cargo's rule for a profile name: letters, digits, `-` and `_`, and not a reserved name.
pub fn validate_name(name: &str) -> Result<()> {
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        bail!("invalid profile name `{name}`: use letters, digits, `-` and `_`");
    }
    if matches!(
        name,
        "debug" | "doc" | "check" | "build" | "package" | "deps"
    ) {
        bail!("profile name `{name}` is reserved");
    }
    Ok(())
}

/// The inheritance chain of `name`, root (`dev` or `release`) first, `name` last.
pub(super) fn chain(loaded: &ResolvedProfiles, name: &str) -> Result<Vec<String>> {
    let mut chain = vec![name.to_string()];
    let mut current = name.to_string();
    while current != "dev" && current != "release" {
        let table = loaded.profiles.get(&current);
        // The built-in test and bench profiles exist without a table.
        let implicit = match current.as_str() {
            "test" => Some("dev"),
            "bench" => Some("release"),
            _ => None,
        };
        if table.is_none() && implicit.is_none() {
            bail!("profile `{current}` is not defined");
        }
        let parent = table
            .and_then(|table| table.get("inherits"))
            .and_then(toml::Value::as_str)
            .or(implicit)
            .ok_or_else(|| {
                anyhow!(
                    "profile `{current}` is missing an `inherits` directive \
                     (`inherits` is required for all profiles except `dev` or `release`)"
                )
            })?;
        if chain.iter().any(|seen| seen == parent) {
            bail!("profile inheritance loop: `{current}` inherits `{parent}`");
        }
        if !matches!(parent, "dev" | "release" | "test" | "bench") {
            validate_name(parent)
                .map_err(|e| anyhow!("profile `{current}` inherits `{parent}`: {e}"))?;
        }
        chain.push(parent.to_string());
        current = parent.to_string();
    }
    chain.reverse();
    Ok(chain)
}

/// Normalizes `opt.profile` and selects it for this process: `dev` / `release` become the
/// plain builds, any other name must be a defined profile, and `opt.release` says whether
/// it derives from `release`. The builder, the planner and the key derivation call this,
/// so library callers get the same result as `--profile`.
pub fn select(opt: &mut crate::adapter::rust::types::RustBuildOptions) -> Result<()> {
    if let Some(name) = opt.profile.clone() {
        match name.as_str() {
            "dev" => {
                opt.release = false;
                opt.profile = None;
            }
            "release" => {
                opt.release = true;
                opt.profile = None;
            }
            _ => {
                validate_name(&name)?;
                let manifest = crate::adapter::rust::RustAdapter::resolve_manifest(opt)?;
                let ws_root = super::super::compiler_resolve::find_workspace_root(&manifest);
                opt.release = inherits_release(&ws_root, &name)?;
            }
        }
    }
    set_custom_profile(opt.profile.as_deref());
    Ok(())
}

/// The tables of every profile in `chain` at one override level, merged root first.
pub(super) fn merged_chain_tables<'t>(
    chain: &[String],
    get: impl Fn(&str) -> Option<&'t toml::Table>,
) -> Option<toml::Table> {
    let mut out: Option<toml::Table> = None;
    for name in chain {
        if let Some(table) = get(name) {
            match out.as_mut() {
                Some(base) => super::merge_table(base, table),
                None => out = Some(table.clone()),
            }
        }
    }
    out
}

/// Whether the custom profile `name` defined for the workspace at `ws_root` derives from
/// `release` (else from `dev`). Errors name an undefined profile, a missing `inherits` or a
/// loop, as Cargo does.
pub fn inherits_release(ws_root: &std::path::Path, name: &str) -> Result<bool> {
    let loaded = super::load_workspace_and_config_profiles(ws_root, name)?;
    Ok(chain(&loaded, name)?.first().map(String::as_str) == Some("release"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profiles(text: &str) -> ResolvedProfiles {
        let value: toml::Value = text.parse().unwrap();
        let mut resolved = ResolvedProfiles::default();
        for (name, table) in value["profile"].as_table().unwrap() {
            resolved
                .profiles
                .insert(name.clone(), table.as_table().unwrap().clone());
        }
        resolved
    }

    #[test]
    fn a_chain_runs_from_its_root_to_the_profile() {
        let loaded = profiles(
            "[profile.ci]\ninherits = \"release\"\n[profile.ci-fast]\ninherits = \"ci\"\n",
        );
        assert_eq!(
            chain(&loaded, "ci-fast").unwrap(),
            ["release", "ci", "ci-fast"]
        );
        assert_eq!(chain(&loaded, "release").unwrap(), ["release"]);
    }

    #[test]
    fn undefined_missing_inherits_and_loops_are_errors() {
        let loaded = profiles(
            "[profile.a]\ninherits = \"b\"\n[profile.b]\ninherits = \"a\"\n[profile.c]\nopt-level = 1\n",
        );
        assert!(chain(&loaded, "nope")
            .unwrap_err()
            .to_string()
            .contains("not defined"));
        assert!(chain(&loaded, "c")
            .unwrap_err()
            .to_string()
            .contains("inherits"));
        assert!(chain(&loaded, "a")
            .unwrap_err()
            .to_string()
            .contains("loop"));
        assert!(validate_name("debug").is_err());
        assert!(validate_name("ci fast").is_err());
        assert!(validate_name("ci-fast_2").is_ok());
    }
}
