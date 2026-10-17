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
use crate::adapter::rust::package_selection::parse_dep_spec;
use crate::adapter::rust::types::{RustAdapterError, RustBuildOptions};
use std::collections::{HashMap, HashSet};
use std::path::Path;

pub struct FeatureGraphIndex<'a> {
    pub explicit_target: Option<String>,
    pub start_dir: &'a Path,
    pub pkg_ids: Vec<String>,
    pub aliases_by_idx: Vec<DependencyAliases>,
    proc_macro_by_id: HashMap<String, bool>,
    by_norm_ver: HashMap<(String, &'a str), String>,
    by_norm_first: HashMap<String, String>,
    ctx_dep_ver: Vec<HashMap<String, Option<&'a str>>>,
    cfg_incompat_cache: HashMap<(String, bool), bool>,
}

impl<'a> FeatureGraphIndex<'a> {
    pub fn build(packages: &'a [PackageFeatureContext<'a>], opt: &'a RustBuildOptions) -> Self {
        let explicit_target = crate::adapter::rust::key_flags::resolve_explicit_target(opt);
        let start_dir = opt.start_dir.as_deref().unwrap_or(Path::new("."));
        let mut pkg_ids = Vec::with_capacity(packages.len());
        let mut aliases_by_idx = Vec::with_capacity(packages.len());
        let mut proc_macro_by_id = HashMap::with_capacity(packages.len());
        let mut by_norm_ver = HashMap::with_capacity(packages.len());
        let mut by_norm_first = HashMap::with_capacity(packages.len());
        let mut ctx_dep_ver = Vec::with_capacity(packages.len());

        for ctx in packages {
            let id = ctx.id();
            let norm_name = ctx.name.replace('-', "_");
            proc_macro_by_id
                .entry(id.clone())
                .or_insert_with(|| ctx.is_proc_macro());
            by_norm_ver
                .entry((norm_name.clone(), ctx.version))
                .or_insert_with(|| id.clone());
            by_norm_first.entry(norm_name).or_insert_with(|| id.clone());
            aliases_by_idx.push(ctx.toml.dependency_aliases());

            let mut dep_map = HashMap::with_capacity(ctx.dependencies.len());
            for s in ctx.dependencies {
                let (name, ver) = parse_dep_spec(s);
                dep_map.entry(name.replace('-', "_")).or_insert(ver);
            }
            ctx_dep_ver.push(dep_map);
            pkg_ids.push(id);
        }

        Self {
            explicit_target,
            start_dir,
            pkg_ids,
            aliases_by_idx,
            proc_macro_by_id,
            by_norm_ver,
            by_norm_first,
            ctx_dep_ver,
            cfg_incompat_cache: HashMap::new(),
        }
    }

    pub fn resolve_dep_id_from_idx(&self, pkg_idx: usize, dep_name: &str) -> Option<String> {
        let clean = dep_name.replace('-', "_");
        let req_ver = self.ctx_dep_ver[pkg_idx].get(&clean).copied().flatten();
        req_ver
            .and_then(|v| self.by_norm_ver.get(&(clean.clone(), v)).cloned())
            .or_else(|| self.by_norm_first.get(&clean).cloned())
    }

    pub fn is_dep_proc_macro(&self, pkg_idx: usize, dep_name: &str) -> bool {
        self.resolve_dep_id_from_idx(pkg_idx, dep_name)
            .and_then(|tid| self.proc_macro_by_id.get(&tid).copied())
            .unwrap_or(false)
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
    ) {
        let aliases = &self.aliases_by_idx[pkg_idx];
        let real_name = match dep {
            TomlDependency::Simple(_) => aliases
                .alias_to_real
                .get(dep_name)
                .map(|s| s.as_str())
                .unwrap_or(dep_name),
            TomlDependency::Detailed(d) => d.package_name.as_deref().unwrap_or_else(|| {
                aliases
                    .alias_to_real
                    .get(dep_name)
                    .map(|s| s.as_str())
                    .unwrap_or(dep_name)
            }),
        };
        let (feats, default_feats) = match dep {
            TomlDependency::Simple(_) => (Vec::new(), true),
            TomlDependency::Detailed(d) => (
                d.features.clone().unwrap_or_default(),
                d.default_features.unwrap_or(true),
            ),
        };

        if let Some(target_id) = self.resolve_dep_id_from_idx(pkg_idx, real_name) {
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
    }

    pub fn activate_manifest_dep(
        &self,
        pkg_idx: usize,
        dep_name: &str,
        deps: &[&TomlDependency],
        active_packages: &mut HashSet<String>,
        requested: &mut HashMap<String, HashSet<String>>,
        changed: &mut bool,
    ) {
        for d in deps {
            self.activate_dep_instance(pkg_idx, dep_name, d, active_packages, requested, changed);
        }
    }
}

pub fn is_implicit_feature(toml: &CargoToml, name: &str) -> bool {
    let (n1, n2) = (
        format!("dep:{}", name),
        format!("dep:{}", name.replace('-', "_")),
    );
    !toml
        .features
        .values()
        .any(|sf| sf.iter().any(|s| s == &n1 || s == &n2))
}

pub fn is_optional_dep(deps: &[&TomlDependency]) -> bool {
    deps.iter().any(|d| match d {
        TomlDependency::Detailed(dt) => dt.optional.unwrap_or(false),
        _ => false,
    })
}

pub fn has_build_dep(toml: &CargoToml, name: &str) -> bool {
    toml.build_dependencies.contains_key(name)
        || toml
            .target
            .values()
            .any(|t| t.build_dependencies.contains_key(name))
}

pub fn is_any_optional_dep(toml: &CargoToml, name: &str) -> bool {
    let norm = name.replace('-', "_");
    let check = |map: &HashMap<String, TomlDependency>| {
        map.get(name)
            .or_else(|| map.get(&norm))
            .is_some_and(|d| match d {
                TomlDependency::Detailed(dt) => dt.optional.unwrap_or(false),
                _ => false,
            })
    };
    check(&toml.dependencies)
        || check(&toml.build_dependencies)
        || toml
            .target
            .values()
            .any(|t| check(&t.dependencies) || check(&t.build_dependencies))
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
