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
