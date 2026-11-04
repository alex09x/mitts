/*
 * Mitts Build — publish and track Rust library aliases.
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::dag_cache::ExecutionContext;
use std::path::Path;

pub(crate) fn publish_library_alias(
    ctx: &ExecutionContext,
    artifact: &Path,
    crate_name: &str,
    extension: &str,
    hash: &str,
) {
    let profile = if ctx.opt.release { "release" } else { "debug" };
    let name = format!("lib{crate_name}.{extension}");
    let relative = format!("{profile}/deps/{name}");
    let alias = ctx.target_dir.join(&relative);
    if alias != artifact
        && crate::core::link_intra_workspace(artifact, &alias, &[ctx.target_dir], false).is_ok()
    {
        if let Some(index) = ctx.target_index {
            index
                .lock()
                .unwrap()
                .record(relative, hash.to_owned(), &alias);
        }
    }
}

/// Links a workspace library's dynamic library or rlib next to the profile's other final
/// outputs, `target/<profile>/lib<crate>.<ext>`, as Cargo does. A consumer that runs a
/// generator on `target/release/libname.dylib` finds it there; the copy in `deps/` is what
/// dependents compile against. The static archive is already placed there.
pub(crate) fn publish_uplifted_library(
    ctx: &ExecutionContext,
    artifact: &Path,
    crate_name: &str,
    extension: &str,
    hash: &str,
) {
    if !matches!(extension, "dylib" | "so" | "rlib") {
        return;
    }
    let profile = if ctx.opt.release { "release" } else { "debug" };
    let relative = format!("{profile}/lib{crate_name}.{extension}");
    let uplifted = ctx.target_dir.join(&relative);
    if uplifted != artifact
        && crate::core::link_intra_workspace(artifact, &uplifted, &[ctx.target_dir], false).is_ok()
    {
        if let Some(index) = ctx.target_index {
            index
                .lock()
                .unwrap()
                .record(relative, hash.to_owned(), &uplifted);
        }
    }
}
