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

/// Points every directive path under `from` (the OUT_DIR the script ran with) at `to`:
/// link and search arguments, scoped link arguments and `rustc-flags`. The spelling the
/// script printed is matched with either separator.
pub fn rebase_out_dir(directives: &mut BuildScriptDirectives, from: &Path, to: &Path) {
    let from_text = from.to_string_lossy().to_string();
    let spellings = [from_text.clone(), from_text.replace('\\', "/")];
    let to_text = to.to_string_lossy().to_string();
    let rebase = |value: &mut String| {
        for spelling in &spellings {
            if !spelling.is_empty() && value.contains(spelling.as_str()) {
                *value = value.replace(spelling.as_str(), &to_text);
            }
        }
    };
    directives.link_args.iter_mut().for_each(rebase);
    directives.link_searches.iter_mut().for_each(rebase);
    directives.flags.iter_mut().for_each(rebase);
    directives
        .scoped_link_args
        .iter_mut()
        .for_each(|(_, arg)| rebase(arg));
}

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
    /// `cargo:rustc-link-arg`: every linked target of the package.
    #[serde(default)]
    pub link_args: Vec<String>,
    /// The scoped forms, `(scope, argument)`: `bins`, `bin:<name>`, `tests`, `benches`,
    /// `examples`, `cdylib` (`rustc-link-arg-bins`, `-bin=<name>=`, `-tests`, `-benches`,
    /// `-examples`, `rustc-cdylib-link-arg`). Cargo passes each to those targets alone.
    #[serde(default)]
    pub scoped_link_args: Vec<(String, String)>,
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
    /// Files (relative to the package, or absolute) and `env:` variables the script's
    /// own compilation read, from its dep-info, with their content or value hashes.
    #[serde(default)]
    pub script_inputs: HashMap<String, String>,
    /// Digest of every package file, recorded only for a mutable package whose script
    /// emitted no rerun directive: Cargo then reruns it when any package file changes.
    #[serde(default)]
    pub package_inputs_digest: Option<String>,
    /// From a `target.<triple>.<links>` override, not from running the script.
    #[serde(default)]
    pub overridden: bool,
    /// `cargo:warning=` lines, kept with the result so a cache hit shows them again, as
    /// Cargo replays them for a fresh build script.
    #[serde(default)]
    pub warnings: Vec<String>,
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
            && self.scoped_link_args.is_empty()
            && self.envs.is_empty()
            && self.metadata.is_empty()
            && self.rerun_if_changed.is_empty()
            && self.rerun_if_env_changed.is_empty()
    }

    /// The link arguments Cargo passes to a unit of `kind` named `target_name`: the plain
    /// ones, then the scoped ones whose scope covers it. A binary's unit test (`<bin>_bin_test`)
    /// is a binary target to Cargo; a library's unit test (`<pkg>_unit_test`) is not a test.
    pub fn link_args_for(&self, kind: &str, target_name: &str) -> Vec<&str> {
        let bin_name = if kind == "bin" {
            Some(target_name)
        } else if kind == "test" {
            target_name.strip_suffix("_bin_test")
        } else {
            None
        };
        let integration_test = kind == "test"
            && !target_name.ends_with("_unit_test")
            && !target_name.ends_with("_bin_test");
        let mut out: Vec<&str> = self.link_args.iter().map(String::as_str).collect();
        for (scope, arg) in &self.scoped_link_args {
            let applies = match scope.as_str() {
                "bins" => bin_name.is_some(),
                "tests" => integration_test,
                "benches" => kind == "bench",
                "examples" => kind == "example",
                "cdylib" => kind.split(',').any(|k| k.trim() == "cdylib"),
                other => match (other.strip_prefix("bin:"), bin_name) {
                    // Test unit names have `-` as `_`.
                    (Some(want), Some(name)) => {
                        want == name || (kind == "test" && want.replace('-', "_") == name)
                    }
                    _ => false,
                },
            };
            if applies {
                out.push(arg);
            }
        }
        out
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
        self.is_fresh_with_missing(pkg_dir, false)
    }

    pub(crate) fn is_fresh_with_missing(&self, pkg_dir: &Path, allow_missing: bool) -> bool {
        if !self.script_inputs_fresh(pkg_dir) {
            return false;
        }
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
                    if !allow_missing || fpath.exists() {
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
        } else if self.rerun_if_env_changed.is_empty() {
            match &self.package_inputs_digest {
                Some(expected) => super::build_script_keys::package_inputs_digest(pkg_dir)
                    .is_ok_and(|cur| cur == *expected),
                None => true,
            }
        } else {
            true
        }
    }

    fn script_inputs_fresh(&self, pkg_dir: &Path) -> bool {
        self.script_inputs.iter().all(|(key, expected)| {
            if let Some(var) = key.strip_prefix("env:") {
                let val = std::env::var(var).unwrap_or_else(|_| "__UNSET__".to_string());
                return Sha256::digest(val.as_bytes()) == *expected;
            }
            let path = if Path::new(key).is_absolute() {
                PathBuf::from(key)
            } else {
                pkg_dir.join(key)
            };
            Sha256::digest_file(&path).is_ok_and(|cur| cur == *expected)
        })
    }

    /// Digest of the inputs `is_fresh` checks beyond rerun directives, so entries
    /// recorded for different script sources or package states never collide.
    pub fn freshness_digest(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(b"freshness_v1:");
        let mut keys: Vec<_> = self.script_inputs.iter().collect();
        keys.sort();
        for (k, v) in keys {
            hasher.update(k.as_bytes());
            hasher.update(b"=");
            hasher.update(v.as_bytes());
            hasher.update(b"\n");
        }
        if let Some(d) = &self.package_inputs_digest {
            hasher.update(b"pkg=");
            hasher.update(d.as_bytes());
        }
        bytes_to_hex(&hasher.finalize())
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
        for (scope, a) in &self.scoped_link_args {
            hasher.update(b"link-arg-");
            hasher.update(scope.as_bytes());
            hasher.update(b":");
            hasher.update(a.as_bytes());
            hasher.update(b"\n");
        }

        let mut sorted_envs: Vec<_> = self.envs.iter().collect();
        sorted_envs.sort_by_key(|(k, _)| *k);
        for (k, v) in sorted_envs {
            hasher.update(b"env:");
            hasher.update(k.as_bytes());
            hasher.update(b"=");
            let norm_v = if let Some(ref out_dir) = self.out_dir {
                let out_str = out_dir.to_string_lossy();
                if v.contains(out_str.as_ref()) {
                    v.replace(out_str.as_ref(), "<OUT_DIR>")
                } else {
                    v.to_string()
                }
            } else {
                v.to_string()
            };
            hasher.update(norm_v.as_bytes());
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

        // An override replaces the script, so whatever it sets is the whole input: the
        // `DEP_<LINKS>_*` metadata it hands to dependents too. A registry dependent hashes
        // no config files, so without this a metadata-only change would be a false hit.
        if self.overridden {
            hasher.update(b"override:");
            hasher.update(self.links.as_deref().unwrap_or("").as_bytes());
            hasher.update(b"\n");
            let mut metadata: Vec<_> = self.metadata.iter().collect();
            metadata.sort();
            for (k, v) in metadata {
                hasher.update(b"metadata:");
                hasher.update(k.as_bytes());
                hasher.update(b"=");
                hasher.update(v.as_bytes());
                hasher.update(b"\n");
            }
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
            } else if let Some(rest) = directive.strip_prefix("rustc-link-arg=") {
                directives.link_args.push(rest.to_string());
            } else if let Some(rest) = directive.strip_prefix("rustc-link-arg-bin=") {
                // `rustc-link-arg-bin=<bin>=<arg>`: ripgrep's `rg=/MANIFEST:EMBED` went to
                // the linker whole, as an input file, until this was scoped.
                if let Some((bin, arg)) = rest.split_once('=') {
                    directives
                        .scoped_link_args
                        .push((format!("bin:{bin}"), arg.to_string()));
                }
            } else if let Some((scope, rest)) = [
                ("bins", "rustc-link-arg-bins="),
                ("tests", "rustc-link-arg-tests="),
                ("benches", "rustc-link-arg-benches="),
                ("examples", "rustc-link-arg-examples="),
                ("cdylib", "rustc-cdylib-link-arg="),
            ]
            .into_iter()
            .find_map(|(scope, prefix)| directive.strip_prefix(prefix).map(|r| (scope, r)))
            {
                directives
                    .scoped_link_args
                    .push((scope.to_string(), rest.to_string()));
            } else if let Some(rest) = directive.strip_prefix("rustc-env=") {
                if let Some((k, v)) = rest.split_once('=') {
                    directives.envs.insert(k.to_string(), v.to_string());
                }
            } else if let Some(rest) = directive.strip_prefix("rerun-if-changed=") {
                directives.rerun_if_changed.push(rest.to_string());
            } else if let Some(rest) = directive.strip_prefix("rerun-if-env-changed=") {
                directives.rerun_if_env_changed.push(rest.to_string());
            } else if let Some(rest) = directive.strip_prefix("warning=") {
                directives.warnings.push(rest.to_string());
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

/// `CARGO_CFG_*` variables from `rustc --print cfg` output: a repeated key such as
/// `target_feature` joins its values with commas, a bare name gets an empty value.
pub fn parse_cfg_lines(stdout: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
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
    map
}

/// Queries `rustc --print cfg` for the specified target and formats standard `CARGO_CFG_*` environment variables.
pub fn get_target_cfgs(target: &str) -> HashMap<String, String> {
    if let Ok(mut lock) = TARGET_CFGS.lock() {
        let cache = lock.get_or_insert_with(HashMap::new);
        if let Some(cfgs) = cache.get(target) {
            return cfgs.clone();
        }
        let rustc = crate::adapter::rust::compiler::resolve_rustc();
        let explicit = (target != "host").then_some(target);
        let map = super::toolchain_probes::rustc_print_cfg(&rustc, explicit)
            .map(|stdout| parse_cfg_lines(&stdout))
            .unwrap_or_default();
        cache.insert(target.to_string(), map.clone());
        map
    } else {
        HashMap::new()
    }
}

#[cfg(test)]
#[path = "build_script_directives_tests.rs"]
mod tests;
