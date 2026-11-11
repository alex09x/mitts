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
use crate::adapter::RustBuildOptions;
use anyhow::{bail, Result};

/// Applies `--profile NAME` to the options, as Cargo does: `dev` and `release` are the
/// usual builds; `test`, `bench` and profiles defined under `[profile.NAME]` with an
/// `inherits` chain are custom, built into `target/NAME` (test/bench: debug/release).
/// `--release` with any profile but `release` is a conflict.
pub fn apply(opt: &mut RustBuildOptions, profile: Option<&str>) -> Result<()> {
    if let Some(name) = profile {
        if opt.release && name != "release" {
            bail!("conflicting usage of --profile={name} and --release");
        }
    }
    opt.profile = profile.map(str::to_string);
    custom::select(opt)
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
