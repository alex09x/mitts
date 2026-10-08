/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub use super::build_script_keys::{
    compute_build_script_key, compute_execution_key, detect_default_target_triple,
    get_rustc_verbose_version,
};
use crate::core::hash::{bytes_to_hex, Sha256};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Output captured from running a `build.rs` build script.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BuildScriptDirectives {
    pub cfgs: Vec<String>,
    #[serde(default)]
    pub check_cfgs: Vec<String>,
    #[serde(default)]
    pub flags: Vec<String>,
    pub link_libs: Vec<String>,
    pub link_searches: Vec<String>,
    #[serde(default)]
    pub link_args: Vec<String>,
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
            && self.check_cfgs.is_empty()
            && self.flags.is_empty()
            && self.link_libs.is_empty()
            && self.link_searches.is_empty()
            && self.link_args.is_empty()
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

    /// Computes a canonical hash of all compile-affecting directives (cfgs, flags, link libs/args/searches, envs).
    pub fn compute_compile_hash(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(b"directives_compile_view_v1:\n");

        let mut cfgs = self.cfgs.clone();
        cfgs.sort();
        cfgs.dedup();
        for c in cfgs {
            hasher.update(b"cfg:");
            hasher.update(c.as_bytes());
            hasher.update(b"\n");
        }

        let mut check_cfgs = self.check_cfgs.clone();
        check_cfgs.sort();
        check_cfgs.dedup();
        for cc in check_cfgs {
            hasher.update(b"check-cfg:");
            hasher.update(cc.as_bytes());
            hasher.update(b"\n");
        }

        let mut flags = self.flags.clone();
        flags.sort();
        flags.dedup();
        for f in flags {
            hasher.update(b"flag:");
            hasher.update(f.as_bytes());
            hasher.update(b"\n");
        }

        let mut link_libs = self.link_libs.clone();
        link_libs.sort();
        link_libs.dedup();
        for l in link_libs {
            hasher.update(b"link-lib:");
            hasher.update(l.as_bytes());
            hasher.update(b"\n");
        }

        let mut link_args = self.link_args.clone();
        link_args.sort();
        link_args.dedup();
        for a in link_args {
            hasher.update(b"link-arg:");
            hasher.update(a.as_bytes());
            hasher.update(b"\n");
        }

        let mut sorted_envs: Vec<_> = self.envs.iter().collect();
        sorted_envs.sort_by_key(|(k, _)| *k);
        for (k, v) in sorted_envs {
            hasher.update(b"env:");
            hasher.update(k.as_bytes());
            hasher.update(b"=");
            hasher.update(v.as_bytes());
            hasher.update(b"\n");
        }

        let mut norm_searches = Vec::new();
        for s in &self.link_searches {
            let path = Path::new(s);
            let norm = if let Some(ref out_dir) = self.out_dir {
                if path == out_dir {
                    "<OUT_DIR>".to_string()
                } else if let Ok(rel) = path.strip_prefix(out_dir) {
                    format!("<OUT_DIR>/{}", rel.display())
                } else {
                    s.clone()
                }
            } else {
                s.clone()
            };
            norm_searches.push(norm);
        }
        norm_searches.sort();
        norm_searches.dedup();
        for s in norm_searches {
            hasher.update(b"link-search:");
            hasher.update(s.as_bytes());
            hasher.update(b"\n");
        }

        bytes_to_hex(&hasher.finalize())
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
            } else if let Some(rest) = directive.strip_prefix("rustc-check-cfg=") {
                directives.check_cfgs.push(rest.to_string());
            } else if let Some(rest) = directive.strip_prefix("rustc-flags=") {
                directives.flags.push(rest.to_string());
            } else if let Some(rest) = directive.strip_prefix("rustc-link-lib=") {
                directives.link_libs.push(rest.to_string());
            } else if let Some(rest) = directive.strip_prefix("rustc-link-search=") {
                let val = rest.strip_prefix("native=").unwrap_or(rest);
                directives.link_searches.push(val.to_string());
            } else if let Some(rest) = directive
                .strip_prefix("rustc-link-arg=")
                .or_else(|| directive.strip_prefix("rustc-link-arg-bins="))
                .or_else(|| directive.strip_prefix("rustc-link-arg-bin="))
                .or_else(|| directive.strip_prefix("rustc-link-arg-tests="))
                .or_else(|| directive.strip_prefix("rustc-link-arg-benches="))
                .or_else(|| directive.strip_prefix("rustc-link-arg-examples="))
                .or_else(|| directive.strip_prefix("rustc-cdylib-link-arg="))
            {
                directives.link_args.push(rest.to_string());
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
                    if !k.starts_with("rerun-") && k != "warning" && !k.starts_with("rustc-") {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_parse_stdout_dedup() {
        let stdout = "cargo:rerun-if-changed=build.rs\ncargo:rerun-if-changed=build.rs\ncargo:rustc-cfg=feat\n";
        let dir = BuildScriptDirectives::parse_stdout(stdout);
        assert_eq!(dir.rerun_if_changed, vec!["build.rs"]);
        assert_eq!(dir.cfgs, vec!["feat"]);
    }

    #[test]
    fn test_compute_compile_hash_differs_on_cfg() {
        let dir1 = BuildScriptDirectives::parse_stdout("cargo:rustc-cfg=fast\n");
        let dir2 = BuildScriptDirectives::parse_stdout("");
        assert_ne!(dir1.compute_compile_hash(), dir2.compute_compile_hash());
    }

    #[test]
    fn test_compute_compile_hash_normalizes_out_dir() {
        let d1 = BuildScriptDirectives {
            out_dir: Some(PathBuf::from("/tmp/stage_run1")),
            link_searches: vec!["/tmp/stage_run1/lib".to_string(), "/usr/lib".to_string()],
            ..Default::default()
        };

        let d2 = BuildScriptDirectives {
            out_dir: Some(PathBuf::from("/tmp/stage_run2")),
            link_searches: vec!["/tmp/stage_run2/lib".to_string(), "/usr/lib".to_string()],
            ..Default::default()
        };

        assert_eq!(d1.compute_compile_hash(), d2.compute_compile_hash());
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
