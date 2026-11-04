/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::dag_cache::ExecutionContext;
use std::path::{Path, PathBuf};

/// Records dep-info extra inputs (`include_str!`, `#[path]`, `env-dep:`) into the ExecutionContext for FastBuildLock.
pub fn record_extra_inputs_in_ctx(
    ctx: &ExecutionContext,
    ws_root: Option<&Path>,
    extra_inputs: &[(String, String)],
) {
    for (path_str, hash) in extra_inputs {
        if let Some(var_name) = path_str.strip_prefix("env:") {
            if var_name == "OUT_DIR"
                || var_name == "CARGO_MANIFEST_DIR"
                || var_name == "CARGO_CRATE_NAME"
                || var_name == "CARGO_BIN_NAME"
                || var_name.starts_with("CARGO_PKG_")
            {
                continue;
            }
            if let Some(env_map) = ctx.tracked_env_vars {
                env_map
                    .lock()
                    .unwrap()
                    .insert(var_name.to_string(), std::env::var(var_name).ok());
            }
        } else if !path_str.starts_with("out:") {
            let file_path = if let Some(rel) = path_str.strip_prefix("ws:") {
                if let Some(root) = ws_root {
                    root.join(rel)
                } else {
                    PathBuf::from(rel)
                }
            } else {
                PathBuf::from(path_str)
            };
            if let Some(files_map) = ctx.tracked_extra_files {
                files_map.lock().unwrap().insert(file_path, hash.clone());
            }
        }
    }
}
