/*
 * Mitts Build — `--profile NAME` on the build commands
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::adapter::rust::profile::custom;
use crate::adapter::{RustAdapter, RustBuildOptions};
use anyhow::{bail, Result};

/// Applies `--profile NAME` to the options, as Cargo does: `dev` and `release` are the
/// usual builds; `test`, `bench` and profiles defined under `[profile.NAME]` with an
/// `inherits` chain are custom, built into `target/NAME` (test/bench: debug/release).
/// `--release` with any profile but `release` is a conflict.
pub fn apply(opt: &mut RustBuildOptions, profile: Option<&str>) -> Result<()> {
    let Some(name) = profile else {
        opt.profile = None;
        return Ok(());
    };
    if opt.release && name != "release" {
        bail!("conflicting usage of --profile={name} and --release");
    }
    match name {
        "dev" => {
            opt.release = false;
            opt.profile = None;
        }
        "release" => {
            opt.release = true;
            opt.profile = None;
        }
        _ => {
            custom::validate_name(name)?;
            let manifest = RustAdapter::resolve_manifest(opt)?;
            let ws_root = crate::adapter::rust::compiler_resolve::find_workspace_root(&manifest);
            opt.release = custom::inherits_release(&ws_root, name)?;
            opt.profile = Some(name.to_string());
        }
    }
    custom::set_custom_profile(opt.profile.as_deref());
    Ok(())
}

/// `apply`, reporting a failure the way the build commands report errors: exit code 101.
pub fn apply_or_exit(opt: &mut RustBuildOptions, profile: Option<&str>) -> Option<i32> {
    match apply(opt, profile) {
        Ok(()) => None,
        Err(error) => {
            eprintln!("error: {error:#}");
            Some(101)
        }
    }
}
