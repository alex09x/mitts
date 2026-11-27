/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::core::hash::Sha256;
use std::collections::HashMap;
use std::path::Path;

/// Reads the build script's dep-info: every source file its compilation read, keyed
/// relative to the package when inside it, and every `env!` variable that mitts does
/// not set itself (those are already part of the build key), as the compile saw it
/// (`[env]` of `env_dir`'s configs, then the environment). `None` when any input cannot be
/// resolved and hashed.
pub fn collect_script_inputs(
    d_path: &Path,
    pkg_dir: &Path,
    env_dir: &Path,
    extra_env: Option<&HashMap<String, String>>,
) -> Option<HashMap<String, String>> {
    let parsed = crate::adapter::rust::dep_info::parse_dep_info_full(d_path, None).ok()?;
    let ws_root = crate::adapter::rust::compiler::find_workspace_root(pkg_dir);
    let mut inputs = HashMap::new();
    for file in parsed.files {
        let p = [file.clone(), pkg_dir.join(&file), ws_root.join(&file)]
            .into_iter()
            .find(|p| p.is_absolute() && p.is_file())?;
        let key = p
            .strip_prefix(pkg_dir)
            .unwrap_or(&p)
            .to_string_lossy()
            .to_string();
        inputs.insert(key, Sha256::digest_file(&p).ok()?);
    }
    for (var, _) in parsed.env_deps {
        if var.starts_with("CARGO_") || extra_env.is_some_and(|e| e.contains_key(&var)) {
            continue;
        }
        let val = crate::adapter::rust::cargo_config::config_env_lookup(env_dir, &var)
            .unwrap_or_else(|| "__UNSET__".to_string());
        inputs.insert(format!("env:{var}"), Sha256::digest(val.as_bytes()));
    }
    Some(inputs)
}
