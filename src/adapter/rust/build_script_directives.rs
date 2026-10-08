/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::adapter::rust::inputs::hash_package_base_inputs;
use crate::adapter::rust::types::PlannedCompilationUnit;
use crate::core::hash::{bytes_to_hex, Sha256};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

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
    pub rerun_if_changed_hashes: HashMap<String, String>,
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

    /// Checks if this cached directive is still valid based on environment variables and file content hashes.
    pub fn is_fresh(&self, pkg_dir: &Path) -> bool {
        // Check rerun-if-env-changed
        for var in &self.rerun_if_env_changed {
            let cur_val = std::env::var(var).ok();
            let cached_val = self.envs.get(var).cloned();
            if cur_val != cached_val {
                return false;
            }
        }
        // Check rerun-if-changed files via content hash (never mtimes)
        if !self.rerun_if_changed.is_empty() || !self.rerun_if_changed_hashes.is_empty() {
            if self.rerun_if_changed.len() != self.rerun_if_changed_hashes.len() {
                return false;
            }
            for (file_rel, expected_hash) in &self.rerun_if_changed_hashes {
                let fpath = if Path::new(file_rel).is_absolute() {
                    PathBuf::from(file_rel)
                } else {
                    pkg_dir.join(file_rel)
                };
                if expected_hash == "<missing>" {
                    if fpath.exists() {
                        return false;
                    }
                } else {
                    if !fpath.exists() {
                        return false;
                    }
                    if let Ok(cur_hash) = Sha256::digest_path_recursive(&fpath) {
                        if cur_hash != *expected_hash {
                            return false;
                        }
                    } else {
                        return false;
                    }
                }
            }
            true
        } else {
            self.rerun_if_changed.is_empty()
        }
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
        directives.rerun_if_changed.sort();
        directives.rerun_if_changed.dedup();
        directives.rerun_if_env_changed.sort();
        directives.rerun_if_env_changed.dedup();
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
        let mut cmd = std::process::Command::new(crate::adapter::rust::compiler::resolve_rustc());
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

static RUSTC_VERBOSE_VERSION: OnceLock<String> = OnceLock::new();

pub fn get_rustc_verbose_version() -> &'static str {
    RUSTC_VERBOSE_VERSION.get_or_init(|| {
        let cmd = std::process::Command::new(crate::adapter::rust::compiler::resolve_rustc()).arg("-vV").output();
        if let Ok(out) = cmd {
            if out.status.success() {
                return String::from_utf8_lossy(&out.stdout).trim().to_string();
            }
        }
        "unknown_rustc".to_string()
    })
}

pub fn compute_build_script_key(
    unit: &PlannedCompilationUnit,
    build_rs: &Path,
    release: bool,
    features: &[String],
    target: &str,
    extra_env: Option<&HashMap<String, String>>,
    extern_crates: &[(String, PathBuf)],
) -> Result<String> {
    let mut hasher = Sha256::new();
    hasher.update(b"build_script_v4:");
    hasher.update(unit.package_name.as_bytes());
    hasher.update(b":");
    hasher.update(unit.version.as_bytes());
    hasher.update(b":");
    hasher.update(unit.edition.as_deref().unwrap_or("2015").as_bytes());
    hasher.update(b":");
    let script_bytes = fs::read(build_rs).with_context(|| {
        format!(
            "Failed to read build script source at {}",
            build_rs.display()
        )
    })?;
    hasher.update(&script_bytes);
    hasher.update(if release { b":release:" } else { b":dev:" });
    hasher.update(target.as_bytes());
    hasher.update(b":rustc:");
    hasher.update(get_rustc_verbose_version().as_bytes());

    for (crate_name, artifact_path) in extern_crates {
        hasher.update(b":build_dep:");
        hasher.update(crate_name.as_bytes());
        hasher.update(b"=");
        let digest = Sha256::digest_file(artifact_path).with_context(|| {
            format!(
                "Failed to hash build-dependency artifact '{}' at {}",
                crate_name,
                artifact_path.display()
            )
        })?;
        hasher.update(digest.as_bytes());
    }

    for f in features {
        hasher.update(b":feat:");
        hasher.update(f.as_bytes());
    }
    if let Some(env) = extra_env {
        let mut sorted: Vec<_> = env.iter().collect();
        sorted.sort_by_key(|(k, _)| *k);
        for (k, v) in sorted {
            hasher.update(b":env:");
            hasher.update(k.as_bytes());
            hasher.update(b"=");
            hasher.update(v.as_bytes());
        }
    }
    if unit.is_workspace {
        let pkg_dir = unit.manifest_path.parent().with_context(|| {
            format!(
                "Manifest path '{}' has no parent directory",
                unit.manifest_path.display()
            )
        })?;
        let base_inputs = hash_package_base_inputs(pkg_dir, None, true).with_context(|| {
            format!(
                "Failed to hash workspace package inputs for '{}' at {}",
                unit.package_name,
                pkg_dir.display()
            )
        })?;
        for (path, digest) in base_inputs {
            hasher.update(b":ws_input:");
            hasher.update(path.as_bytes());
            hasher.update(b"=");
            hasher.update(digest.as_bytes());
        }
    }
    Ok(bytes_to_hex(&hasher.finalize()))
}

/// Computes an execution cache key combining the base build_key with sorted rerun-if-changed file hashes
/// and sorted rerun-if-env-changed values.
pub fn compute_execution_key(
    base_key: &str,
    rerun_if_changed_hashes: &HashMap<String, String>,
    envs: &HashMap<String, String>,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"exec_v2:");
    hasher.update(base_key.as_bytes());

    let mut sorted_files: Vec<_> = rerun_if_changed_hashes.iter().collect();
    sorted_files.sort_by_key(|(k, _)| *k);
    for (k, v) in sorted_files {
        hasher.update(b":file:");
        hasher.update(k.as_bytes());
        hasher.update(b"=");
        hasher.update(v.as_bytes());
    }

    let mut sorted_envs: Vec<_> = envs.iter().collect();
    sorted_envs.sort_by_key(|(k, _)| *k);
    for (k, v) in sorted_envs {
        hasher.update(b":env:");
        hasher.update(k.as_bytes());
        hasher.update(b"=");
        hasher.update(v.as_bytes());
    }

    bytes_to_hex(&hasher.finalize())
}

/// Detects the host target triple based on compile-time target attributes or TARGET env.
pub fn detect_default_target_triple() -> String {
    if let Ok(t) = std::env::var("TARGET") {
        if !t.is_empty() {
            return t;
        }
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    return "aarch64-apple-darwin".to_string();
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    return "x86_64-apple-darwin".to_string();
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    return "x86_64-unknown-linux-gnu".to_string();
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    return "aarch64-unknown-linux-gnu".to_string();
    #[cfg(not(any(
        all(target_os = "macos", target_arch = "aarch64"),
        all(target_os = "macos", target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "aarch64"),
    )))]
    return "host".to_string();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_stdout_dedup() {
        let stdout = "cargo:rerun-if-changed=build.rs\ncargo:rerun-if-changed=build.rs\ncargo:rustc-cfg=feat\n";
        let dir = BuildScriptDirectives::parse_stdout(stdout);
        assert_eq!(dir.rerun_if_changed, vec!["build.rs"]);
        assert_eq!(dir.cfgs, vec!["feat"]);
    }

    #[test]
    fn test_is_fresh_with_missing_file() {
        let mut dir = BuildScriptDirectives::default();
        dir.rerun_if_changed
            .push("nonexistent_header.h".to_string());
        dir.rerun_if_changed_hashes
            .insert("nonexistent_header.h".to_string(), "<missing>".to_string());
        let temp = tempfile::tempdir().unwrap();
        assert!(dir.is_fresh(temp.path()));

        // If the file suddenly appears, freshness must become false
        fs::write(temp.path().join("nonexistent_header.h"), "now exists").unwrap();
        assert!(!dir.is_fresh(temp.path()));
    }
}
