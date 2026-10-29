/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Doctest planning: resolves `rustdoc --test` inputs from the exact artifacts of a build.

use super::dag_unit_externs::resolve_extern_crates;
use super::manifest::CargoToml;
use super::types::{PlannedCompilationUnit, RustBuildOptions};
use super::unit_env::compute_unit_rustc_env;
use crate::adapter::RustAdapter;
use crate::core::action::BuildPlan;
use anyhow::Result;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

/// A fully resolved `rustdoc --test` invocation for one library target.
#[derive(Debug, Clone, Default)]
pub struct DoctestInvocation {
    pub crate_name: String,
    pub crate_type: String,
    pub src_path: PathBuf,
    pub package_dir: PathBuf,
    pub edition: String,
    /// Enabled features of the library unit, passed as `--cfg feature="..."`.
    pub features: Vec<String>,
    /// Additional cfgs from build script directives or plan.
    pub cfgs: Vec<String>,
    /// Check-cfg declarations.
    pub check_cfgs: Vec<String>,
    /// Exact `--extern name=path` pairs: the library itself plus its normal and dev-dependencies.
    pub externs: Vec<(String, PathBuf)>,
    /// `-L dependency=` directories holding the externs (for transitive crate lookup).
    pub search_dirs: Vec<PathBuf>,
    pub env: HashMap<String, String>,
    /// Native link-search directories from build scripts (e.g. `native=/path` or `/path`).
    pub native_search: Vec<String>,
    /// Planned library dependencies that have no built artifact in the plan.
    pub missing: Vec<String>,
    /// False when the library has no unit-test target, so dev-dependencies could not be resolved.
    pub dev_deps_resolved: bool,
}

/// Plans doctest invocations for every primary library in scope.
///
/// Dependencies come from the planner's view of the library's unit-test target
/// (normal + dev-dependencies, as Cargo links doctests); artifact paths come from
/// `plan.artifacts`, i.e. exactly what the build produced. No directory scanning.
pub fn plan_doctests(opt: &RustBuildOptions, plan: &BuildPlan) -> Result<Vec<DoctestInvocation>> {
    let mut plan_opt = opt.clone();
    plan_opt.progress = None;
    plan_opt.doc_only = false;
    plan_opt.lib_only = true;
    plan_opt.test_targets.clear();
    plan_opt.all_tests = false;
    plan_opt.bin_targets.clear();
    plan_opt.all_bins = false;
    let explanation = RustAdapter::explain_plan(&plan_opt)?;

    let completed: HashMap<String, PathBuf> = plan
        .artifacts
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    let mut out = Vec::new();
    for lib in explanation
        .units
        .iter()
        .filter(|u| {
            u.is_primary
                && !u.for_host
                && (super::planner_units::is_library_kind(&u.kind) || u.kind == "proc-macro")
        })
    {
        if let Some(inv) = doctest_for_lib(
            lib,
            &explanation.units,
            &completed,
            &plan.unit_cfgs,
            &plan.unit_env,
            &plan.unit_link_args,
        )? {
            out.push(inv);
        }
    }
    Ok(out)
}

/// Builds the doctest invocation for one library unit, or `None` if doctests are disabled.
pub fn doctest_for_lib(
    lib: &PlannedCompilationUnit,
    units: &[PlannedCompilationUnit],
    completed: &HashMap<String, PathBuf>,
    unit_cfgs: &BTreeMap<String, Vec<String>>,
    unit_env: &BTreeMap<String, HashMap<String, String>>,
    unit_link_args: &BTreeMap<String, Vec<String>>,
) -> Result<Option<DoctestInvocation>> {
    let package_dir = lib
        .manifest_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let toml = CargoToml::from_file(&lib.manifest_path)?;
    if toml.discover_doctest_target(&package_dir).is_none() {
        return Ok(None);
    }

    let is_proc_macro = lib.kind == "proc-macro"
        || toml.lib.as_ref().is_some_and(|l| {
            l.proc_macro == Some(true) || l.crate_type.iter().any(|k| k == "proc-macro")
        });
    let crate_type = if is_proc_macro {
        "proc-macro".to_string()
    } else {
        "lib".to_string()
    };

    let crate_name = lib.target_name.replace('-', "_");
    let unit_test_name = format!("{}_unit_test", lib.package_name.replace('-', "_"));
    let unit_test = units.iter().find(|u| {
        u.package_id == lib.package_id
            && (u.kind == "test" || u.kind == "custom-test")
            && u.target_name == unit_test_name
            && !u.for_host
    });
    let dev_deps_resolved = unit_test.is_some();
    let deps_unit = unit_test
        .cloned()
        .unwrap_or_else(|| PlannedCompilationUnit {
            kind: "test".to_string(),
            target_name: unit_test_name,
            ..lib.clone()
        });

    let mut missing: Vec<String> = deps_unit
        .dependencies
        .iter()
        .filter(|d| is_library_unit(d) && lookup(completed, d).is_none())
        .cloned()
        .collect();

    let mut externs = Vec::new();
    match lookup(completed, &lib.unit_id()) {
        Some(p) => externs.push((crate_name.clone(), linkable(p))),
        None => missing.insert(0, lib.unit_id()),
    }
    for (name, path) in resolve_extern_crates(&deps_unit, completed) {
        if name != crate_name {
            externs.push((name, linkable(&path)));
        }
    }

    let search_dirs: BTreeSet<PathBuf> = externs
        .iter()
        .filter_map(|(_, p)| p.parent().map(Path::to_path_buf))
        .collect();

    let mut features = lib.features.clone();
    features.sort();
    features.dedup();

    let mut check_cfgs = vec!["cfg(docsrs,test)".to_string()];
    if let Some(ref lints_val) = toml.lints {
        for cc in super::manifest_inherit::extract_check_cfg_from_lints(lints_val) {
            check_cfgs.push(cc);
        }
    }
    let mut all_features: Vec<String> = toml.features.keys().cloned().collect();
    for (dep_name, dep_val) in &toml.dependencies {
        if let super::manifest::TomlDependency::Detailed(ref d) = dep_val {
            if d.optional == Some(true) {
                all_features.push(dep_name.clone());
            }
        }
    }
    for target_spec in toml.target.values() {
        for (dep_name, dep_val) in &target_spec.dependencies {
            if let super::manifest::TomlDependency::Detailed(ref d) = dep_val {
                if d.optional == Some(true) {
                    all_features.push(dep_name.clone());
                }
            }
        }
    }
    all_features.sort();
    all_features.dedup();
    let quoted: Vec<String> = all_features.iter().map(|f| format!("\"{}\"", f)).collect();
    check_cfgs.push(format!("cfg(feature, values({}))", quoted.join(", ")));

    let cfgs = unit_cfgs.get(&lib.unit_id()).cloned().unwrap_or_default();
    let mut env = compute_unit_rustc_env(lib, None, None);
    if let Some(extra_env) = unit_env.get(&lib.unit_id()) {
        env.extend(extra_env.clone());
    }

    // In Cargo, rustdoc receives -L (link-search) directories from all build scripts
    // (compilation.native_dirs), preserving any kind prefix (e.g. native=/path),
    // but does NOT receive -l (link-lib) or -C link-arg.
    let mut native_search = Vec::new();
    for args in unit_link_args.values() {
        let mut it = args.iter();
        while let Some(arg) = it.next() {
            if arg == "-L" {
                if let Some(val) = it.next() {
                    if !native_search.contains(val) {
                        native_search.push(val.clone());
                    }
                }
            } else if let Some(val) = arg.strip_prefix("-L") {
                let val_str = val.to_string();
                if !native_search.contains(&val_str) {
                    native_search.push(val_str);
                }
            }
        }
    }

    Ok(Some(DoctestInvocation {
        crate_name,
        crate_type,
        src_path: lib.src_path.clone(),
        package_dir,
        edition: lib.edition.clone().unwrap_or_else(|| "2015".to_string()),
        features,
        cfgs,
        check_cfgs,
        externs,
        search_dirs: search_dirs.into_iter().collect(),
        env,
        native_search,
        missing,
        dev_deps_resolved,
    }))
}

/// True for unit ids of library-like targets (`pkg@ver::lib:name`, `...::proc-macro:name:host`).
fn is_library_unit(unit_id: &str) -> bool {
    let kind = unit_id
        .split("::")
        .nth(1)
        .and_then(|rest| rest.split(':').next())
        .unwrap_or("");
    super::planner_units::is_library_kind(kind) || kind == "proc-macro"
}

/// Looks up a unit artifact, tolerating the host/target split used for dual-compiled crates.
fn lookup<'a>(completed: &'a HashMap<String, PathBuf>, unit_id: &str) -> Option<&'a PathBuf> {
    completed
        .get(unit_id)
        .or_else(|| match unit_id.strip_suffix(":host") {
            Some(target_id) => completed.get(target_id),
            None => completed.get(&format!("{}:host", unit_id)),
        })
}

/// Doctests link against dependencies, so prefer the `.rlib` over pipelined `.rmeta`.
fn linkable(path: &Path) -> PathBuf {
    if path.extension().and_then(|e| e.to_str()) == Some("rmeta") {
        let rlib = path.with_extension("rlib");
        if rlib.is_file() {
            return rlib;
        }
    }
    path.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_library_unit_kinds() {
        assert!(is_library_unit("serde@1.0.0::lib:serde"));
        assert!(is_library_unit(
            "serde_derive@1.0.0::proc-macro:serde_derive:host"
        ));
        assert!(!is_library_unit("foo@0.1.0::test:foo_unit_test"));
        assert!(!is_library_unit("foo@0.1.0::bin:foo"));
    }

    #[test]
    fn test_lookup_host_fallback() {
        let mut m = HashMap::new();
        m.insert("a@1::lib:a".to_string(), PathBuf::from("/x/liba.rlib"));
        m.insert(
            "b@1::proc-macro:b:host".to_string(),
            PathBuf::from("/x/libb.so"),
        );
        assert!(lookup(&m, "a@1::lib:a:host").is_some());
        assert!(lookup(&m, "b@1::proc-macro:b").is_some());
        assert!(lookup(&m, "c@1::lib:c").is_none());
    }
}
