/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::features::PackageFeatureContext;
use crate::adapter::rust::manifest::{CargoToml, TomlDependency};
use crate::adapter::rust::manifest_models::DependencyAliases;
pub use crate::adapter::rust::package_selection::{
    format_package_id, has_build_dep, is_any_optional_dep, is_implicit_feature, is_optional_dep,
    normalize_unit_id_without_source, parse_dep_spec, parse_package_id_source,
};
use crate::adapter::rust::types::{RustAdapterError, RustBuildOptions};
use std::collections::{HashMap, HashSet};
use std::path::Path;

fn semver_or_prefix_matches(req_str: &str, ver_str: &str) -> bool {
    if let (Ok(req), Ok(ver)) = (
        semver::VersionReq::parse(req_str),
        semver::Version::parse(ver_str),
    ) {
        req.matches(&ver)
    } else {
        ver_str.starts_with(req_str) || req_str == ver_str
    }
}

fn is_registry_source(s: &str) -> bool {
    s.starts_with("registry+") || s.starts_with("sparse+")
}

fn matches_toml_dep_hint(cand: &PackageFeatureContext, dep: &TomlDependency) -> bool {
    match dep {
        TomlDependency::Simple(req_str) => {
            semver_or_prefix_matches(req_str, cand.version)
                && cand.source.is_none_or(is_registry_source)
        }
        TomlDependency::Detailed(dt) => {
            let ver_ok = dt
                .version
                .as_deref()
                .is_none_or(|req_str| semver_or_prefix_matches(req_str, cand.version));
            let src_ok = if dt.path.is_some() {
                cand.source.is_none()
            } else if dt.git.is_some() {
                cand.source.is_some_and(|s| s.starts_with("git+"))
            } else {
                cand.source.is_some_and(is_registry_source)
            };
            ver_ok && src_ok
        }
    }
}

type DepSpecEntry<'a> = (Option<&'a str>, Option<&'a str>);

pub struct FeatureGraphIndex<'a> {
    pub packages: &'a [PackageFeatureContext<'a>],
    pub explicit_target: Option<String>,
    pub start_dir: &'a Path,
    pub pkg_ids: Vec<String>,
    pub aliases_by_idx: Vec<DependencyAliases>,
    proc_macro_by_id: HashMap<String, bool>,
    candidates_by_norm: HashMap<String, Vec<usize>>,
    ctx_dep_specs: Vec<HashMap<String, Vec<DepSpecEntry<'a>>>>,
    cfg_incompat_cache: HashMap<(String, bool), bool>,
}

impl<'a> FeatureGraphIndex<'a> {
    pub fn build(packages: &'a [PackageFeatureContext<'a>], opt: &'a RustBuildOptions) -> Self {
        let explicit_target = crate::adapter::rust::key_flags::resolve_explicit_target(opt);
        let start_dir = opt.start_dir.as_deref().unwrap_or(Path::new("."));
        let mut pkg_ids = Vec::with_capacity(packages.len());
        let mut aliases_by_idx = Vec::with_capacity(packages.len());
        let mut proc_macro_by_id = HashMap::with_capacity(packages.len());
        let mut candidates_by_norm: HashMap<String, Vec<usize>> =
            HashMap::with_capacity(packages.len());
        let mut ctx_dep_specs = Vec::with_capacity(packages.len());

        for (idx, ctx) in packages.iter().enumerate() {
            let id = ctx.id();
            let norm_name = ctx.name.replace('-', "_");
            proc_macro_by_id
                .entry(id.clone())
                .or_insert_with(|| ctx.is_proc_macro());
            candidates_by_norm.entry(norm_name).or_default().push(idx);
            aliases_by_idx.push(ctx.toml.dependency_aliases());

            let mut dep_map: HashMap<String, Vec<DepSpecEntry<'a>>> =
                HashMap::with_capacity(ctx.dependencies.len());
            for s in ctx.dependencies {
                let (name, ver, src) = parse_dep_spec(s);
                let entry = dep_map.entry(name.replace('-', "_")).or_default();
                if !entry.contains(&(ver, src)) {
                    entry.push((ver, src));
                }
            }
            ctx_dep_specs.push(dep_map);
            pkg_ids.push(id);
        }

        Self {
            packages,
            explicit_target,
            start_dir,
            pkg_ids,
            aliases_by_idx,
            proc_macro_by_id,
            candidates_by_norm,
            ctx_dep_specs,
            cfg_incompat_cache: HashMap::new(),
        }
    }

    fn resolve_single_spec(
        &self,
        pkg_idx: usize,
        dep_name: &str,
        req_ver: Option<&str>,
        req_src: Option<&str>,
        cand_indices: &[usize],
    ) -> Result<Option<usize>, RustAdapterError> {
        let idx = crate::adapter::rust::dependency_check::resolve_candidate_by_spec(
            cand_indices,
            |&i| {
                let p = &self.packages[i];
                (p.version, p.source)
            },
            req_ver,
            req_src,
            self.packages[pkg_idx].source,
            true,
            dep_name,
        )
        .map_err(RustAdapterError::MetadataFailed)?;
        Ok(idx.map(|i| cand_indices[i]))
    }

    pub fn resolve_dep_id_with_hint(
        &self,
        pkg_idx: usize,
        dep_name: &str,
        dep_hint: Option<&TomlDependency>,
    ) -> Result<Option<String>, RustAdapterError> {
        let aliases = &self.aliases_by_idx[pkg_idx];
        let real_name = match dep_hint {
            Some(TomlDependency::Detailed(d)) => d.package_name.as_deref().unwrap_or_else(|| {
                aliases
                    .alias_to_real
                    .get(dep_name)
                    .map(|s| s.as_str())
                    .unwrap_or(dep_name)
            }),
            _ => aliases
                .alias_to_real
                .get(dep_name)
                .map(|s| s.as_str())
                .unwrap_or(dep_name),
        };
        let clean = real_name.replace('-', "_");
        let Some(cand_indices) = self.candidates_by_norm.get(&clean) else {
            return Ok(None);
        };
        if cand_indices.is_empty() {
            return Ok(None);
        }

        if let Some(specs) = self.ctx_dep_specs[pkg_idx].get(&clean) {
            if specs.len() == 1 {
                let (req_ver, req_src) = specs[0];
                return Ok(self
                    .resolve_single_spec(pkg_idx, real_name, req_ver, req_src, cand_indices)?
                    .map(|i| self.pkg_ids[i].clone()));
            }
            let mut resolved_cands = Vec::new();
            for &(req_ver, req_src) in specs {
                if let Some(i) =
                    self.resolve_single_spec(pkg_idx, real_name, req_ver, req_src, cand_indices)?
                {
                    if !resolved_cands.contains(&i) {
                        resolved_cands.push(i);
                    }
                }
            }
            if resolved_cands.is_empty() {
                return Ok(None);
            }
            if resolved_cands.len() == 1 {
                return Ok(Some(self.pkg_ids[resolved_cands[0]].clone()));
            }
            if let Some(hint) = dep_hint {
                let hinted: Vec<usize> = resolved_cands
                    .iter()
                    .copied()
                    .filter(|&i| matches_toml_dep_hint(&self.packages[i], hint))
                    .collect();
                if hinted.len() == 1 {
                    return Ok(Some(self.pkg_ids[hinted[0]].clone()));
                }
            }
            return Err(RustAdapterError::MetadataFailed(format!(
                "Ambiguous dependency '{}' in package '{}' ({} matching packages)",
                real_name,
                self.pkg_ids[pkg_idx],
                resolved_cands.len()
            )));
        }

        if cand_indices.len() == 1 {
            return Ok(Some(self.pkg_ids[cand_indices[0]].clone()));
        }
        if let Some(hint) = dep_hint {
            let hinted: Vec<usize> = cand_indices
                .iter()
                .copied()
                .filter(|&i| matches_toml_dep_hint(&self.packages[i], hint))
                .collect();
            if hinted.len() == 1 {
                return Ok(Some(self.pkg_ids[hinted[0]].clone()));
            }
        }
        Err(RustAdapterError::MetadataFailed(format!(
            "Ambiguous dependency '{}' in package '{}' ({} candidate packages and no lockfile disambiguation)",
            real_name,
            self.pkg_ids[pkg_idx],
            cand_indices.len()
        )))
    }

    pub fn is_dep_proc_macro_with_hint(
        &self,
        pkg_idx: usize,
        dep_name: &str,
        dep_hint: Option<&TomlDependency>,
    ) -> Result<bool, RustAdapterError> {
        Ok(self
            .resolve_dep_id_with_hint(pkg_idx, dep_name, dep_hint)?
            .and_then(|tid| self.proc_macro_by_id.get(&tid).copied())
            .unwrap_or(false))
    }

    pub fn is_target_incompat(
        &mut self,
        spec: &str,
        for_host: bool,
    ) -> Result<bool, RustAdapterError> {
        let key = (spec.to_string(), for_host);
        if let Some(&cached) = self.cfg_incompat_cache.get(&key) {
            return Ok(cached);
        }
        let res = crate::adapter::rust::target_filter::is_target_cfg_incompatible_with_target(
            spec,
            for_host,
            self.explicit_target.as_deref(),
            self.start_dir,
        )?;
        self.cfg_incompat_cache.insert(key, res);
        Ok(res)
    }

    pub fn collect_manifest_dependencies<'t>(
        &mut self,
        toml: &'t CargoToml,
        name: &str,
    ) -> Result<Vec<&'t TomlDependency>, RustAdapterError> {
        let norm = name.replace('-', "_");
        let mut result = Vec::new();
        let mut add = |map: &'t HashMap<String, TomlDependency>| {
            if let Some(d) = map.get(name).or_else(|| map.get(&norm)) {
                result.push(d);
            }
        };
        add(&toml.dependencies);
        add(&toml.build_dependencies);
        for (spec, t) in &toml.target {
            if !self.is_target_incompat(spec, false)? {
                add(&t.dependencies);
            }
            if !self.is_target_incompat(spec, true)? {
                add(&t.build_dependencies);
            }
        }
        Ok(result)
    }

    pub fn activate_dep_instance(
        &self,
        pkg_idx: usize,
        dep_name: &str,
        dep: &TomlDependency,
        active_packages: &mut HashSet<String>,
        requested: &mut HashMap<String, HashSet<String>>,
        changed: &mut bool,
    ) -> Result<(), RustAdapterError> {
        let (feats, default_feats) = match dep {
            TomlDependency::Simple(_) => (Vec::new(), true),
            TomlDependency::Detailed(d) => (
                d.features.clone().unwrap_or_default(),
                d.default_features.unwrap_or(true),
            ),
        };

        if let Some(target_id) = self.resolve_dep_id_with_hint(pkg_idx, dep_name, Some(dep))? {
            if active_packages.insert(target_id.clone()) {
                *changed = true;
            }
            let set = requested.entry(target_id).or_default();
            if default_feats && set.insert("default".to_string()) {
                *changed = true;
            }
            for f in feats {
                if set.insert(f) {
                    *changed = true;
                }
            }
        }
        Ok(())
    }

    pub fn activate_manifest_dep(
        &self,
        pkg_idx: usize,
        dep_name: &str,
        deps: &[&TomlDependency],
        active_packages: &mut HashSet<String>,
        requested: &mut HashMap<String, HashSet<String>>,
        changed: &mut bool,
    ) -> Result<(), RustAdapterError> {
        for d in deps {
            self.activate_dep_instance(pkg_idx, dep_name, d, active_packages, requested, changed)?;
        }
        Ok(())
    }
}

pub fn finalize_feature_map(
    packages: &[PackageFeatureContext],
    pkg_ids: &[String],
    mut requested: HashMap<String, HashSet<String>>,
    _active_packages: &HashSet<String>,
) -> HashMap<String, Vec<String>> {
    let mut result = HashMap::new();
    for (idx, ctx) in packages.iter().enumerate() {
        let id = &pkg_ids[idx];
        let feats = requested.remove(id).unwrap_or_default();
        let mut list: Vec<String> = Vec::new();
        for f in feats {
            if f.starts_with("dep:")
                || (f == "default" && !ctx.toml.features.contains_key("default"))
            {
                continue;
            }
            if ctx.toml.features.contains_key(&f)
                || (is_any_optional_dep(ctx.toml, &f) && is_implicit_feature(ctx.toml, &f))
            {
                list.push(f);
            }
        }
        list.sort();
        list.dedup();
        result.insert(id.clone(), list.clone());
        result.entry(ctx.name.to_string()).or_insert(list);
    }
    result
}
