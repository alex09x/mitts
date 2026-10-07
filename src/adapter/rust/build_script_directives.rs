/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Output captured from running a `build.rs` build script.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BuildScriptDirectives {
    pub cfgs: Vec<String>,
    pub link_libs: Vec<String>,
    pub link_searches: Vec<String>,
    pub envs: HashMap<String, String>,
    pub out_dir: Option<PathBuf>,
    #[serde(default)]
    pub metadata: HashMap<String, String>,
    #[serde(default)]
    pub links: Option<String>,
    #[serde(default)]
    pub rerun_if_changed: Vec<String>,
    #[serde(default)]
    pub rerun_if_env_changed: Vec<String>,
}

impl BuildScriptDirectives {
    /// Returns true if all directive collections are empty.
    pub fn is_empty(&self) -> bool {
        self.cfgs.is_empty()
            && self.link_libs.is_empty()
            && self.link_searches.is_empty()
            && self.envs.is_empty()
            && self.metadata.is_empty()
            && self.rerun_if_changed.is_empty()
            && self.rerun_if_env_changed.is_empty()
    }

    /// Generates DEP_<LINKS>_<KEY>=<VALUE> environment variables from this build script's metadata.
    pub fn dep_env_vars(&self) -> HashMap<String, String> {
        let mut vars = HashMap::new();
        if let Some(ref links) = self.links {
            let prefix = format!("DEP_{}_", links.to_uppercase().replace('-', "_"));
            for (k, v) in &self.metadata {
                let key = format!("{}{}", prefix, k.to_uppercase().replace('-', "_"));
                vars.insert(key, v.clone());
            }
        }
        vars
    }

    /// Parses cargo: build script directives from raw stdout.
    pub fn parse_stdout(stdout: &str) -> Self {
        let mut directives = Self::default();
        for line in stdout.lines() {
            let trimmed = line.trim();
            let directive = if let Some(rest) = trimmed.strip_prefix("cargo::") {
                rest
            } else if let Some(rest) = trimmed.strip_prefix("cargo:") {
                rest
            } else {
                continue;
            };

            if let Some(rest) = directive.strip_prefix("rustc-cfg=") {
                directives.cfgs.push(rest.to_string());
            } else if let Some(rest) = directive.strip_prefix("rustc-link-lib=") {
                directives.link_libs.push(rest.to_string());
            } else if let Some(rest) = directive.strip_prefix("rustc-link-search=") {
                let val = rest.strip_prefix("native=").unwrap_or(rest);
                directives.link_searches.push(val.to_string());
            } else if let Some(rest) = directive.strip_prefix("rustc-env=") {
                if let Some((k, v)) = rest.split_once('=') {
                    directives.envs.insert(k.to_string(), v.to_string());
                }
            } else if let Some(rest) = directive.strip_prefix("rerun-if-changed=") {
                directives.rerun_if_changed.push(rest.to_string());
            } else if let Some(rest) = directive.strip_prefix("rerun-if-env-changed=") {
                directives.rerun_if_env_changed.push(rest.to_string());
            } else {
                let effective = directive.strip_prefix("metadata=").unwrap_or(directive);
                if let Some((k, v)) = effective.split_once('=') {
                    if !k.starts_with("rerun-")
                        && k != "warning"
                        && k != "rustc-check-cfg"
                        && k != "rustc-flags"
                    {
                        directives.metadata.insert(k.to_string(), v.to_string());
                    }
                }
            }
        }
        directives
    }
}

static TARGET_CFGS: std::sync::Mutex<Option<HashMap<String, HashMap<String, String>>>> =
    std::sync::Mutex::new(None);

/// Queries `rustc --print cfg` for the specified target and formats standard `CARGO_CFG_*` environment variables.
pub fn get_target_cfgs(target: &str) -> HashMap<String, String> {
    if let Ok(mut lock) = TARGET_CFGS.lock() {
        let cache = lock.get_or_insert_with(HashMap::new);
        if let Some(cfgs) = cache.get(target) {
            return cfgs.clone();
        }
        let mut map = HashMap::new();
        let mut cmd = std::process::Command::new("rustc");
        cmd.arg("--print").arg("cfg");
        if target != "host" {
            cmd.arg("--target").arg(target);
        }
        if let Ok(output) = cmd.output() {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                for line in stdout.lines() {
                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }
                    if let Some((k, v)) = line.split_once('=') {
                        let key = format!("CARGO_CFG_{}", k.trim().to_ascii_uppercase());
                        let val = v.trim().trim_matches('"');
                        map.entry(key)
                            .and_modify(|existing: &mut String| {
                                existing.push(',');
                                existing.push_str(val);
                            })
                            .or_insert_with(|| val.to_string());
                    } else {
                        let key = format!("CARGO_CFG_{}", line.to_ascii_uppercase());
                        map.entry(key).or_insert_with(String::new);
                    }
                }
            }
        }
        cache.insert(target.to_string(), map.clone());
        map
    } else {
        HashMap::new()
    }
}
