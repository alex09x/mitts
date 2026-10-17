/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::manifest::{CargoToml, TomlBuild, TomlDependency};
use super::target_filter::is_target_cfg_incompatible_with_target;
use super::types::RustAdapterError;
use std::collections::HashMap;
use std::path::Path;

/// Per-pass memoization table for `(target_spec, for_host)` target-cfg compatibility checks.
pub type TargetCfgCache = HashMap<(String, bool), Result<bool, RustAdapterError>>;

fn eval_target_cfg_incompat(
    target_spec: &str,
    for_host: bool,
    explicit_target: Option<&str>,
    start_dir: &Path,
    cache: &mut TargetCfgCache,
) -> Result<bool, RustAdapterError> {
    let key = (target_spec.to_string(), for_host);
    if let Some(res) = cache.get(&key) {
        return res.clone();
    }
    let res =
        is_target_cfg_incompatible_with_target(target_spec, for_host, explicit_target, start_dir);
    cache.insert(key, res.clone());
    res
}

/// Parses a dependency spec string from `Cargo.lock` into `(name, version, source)`:
/// - `"foo"` -> `("foo", None, None)`
/// - `"foo 1.0.0"` -> `("foo", Some("1.0.0"), None)`
/// - `"foo 1.0.0 (registry+https://...)"` -> `("foo", Some("1.0.0"), Some("registry+https://..."))`
pub fn parse_package_spec(spec: &str) -> (&str, Option<&str>, Option<&str>) {
    let s = spec.trim();
    let Some((name, after_name)) = s.split_once(char::is_whitespace) else {
        return (s, None, None);
    };
    let after_name = after_name.trim_start();
    if after_name.is_empty() {
        return (name, None, None);
    }
    let Some((version, after_ver)) = after_name.split_once(char::is_whitespace) else {
        return (name, Some(after_name), None);
    };
    let after_ver = after_ver.trim();
    let source = after_ver
        .strip_prefix('(')
        .and_then(|inner| inner.strip_suffix(')'))
        .or(if after_ver.is_empty() {
            None
        } else {
            Some(after_ver)
        });
    (name, Some(version), source)
}

/// Resolves a `Cargo.lock` dependency spec `(req_ver, req_src)` against candidates
/// of the same package name using `(version, source)`.
///
/// Disambiguation order when multiple candidates match `(req_ver, req_src)`:
/// 1. Exact single match -> `Ok(Some(idx))`.
/// 2. Explicit `req_ver` without `req_src` (`"foo 1.0.0"`) -> unique local/path candidate (`source = None`).
/// 3. Implicit `req_ver = None, req_src = None` (`"foo"`) when all matches share the same version ->
///    unique candidate matching `parent_source`.
/// 4. Otherwise -> `Err`. When `fail_on_no_match && !candidates.is_empty()` and `0` candidates matched,
///    also returns `Err` (inconsistent lockfile edge).
pub fn resolve_candidate_by_spec<'a, T, F>(
    candidates: &'a [T],
    get_ver_src: F,
    req_ver: Option<&str>,
    req_src: Option<&str>,
    parent_source: Option<&str>,
    fail_on_no_match: bool,
    dep_label: &str,
) -> Result<Option<usize>, String>
where
    F: Fn(&'a T) -> (&'a str, Option<&'a str>),
{
    let matches: Vec<usize> = candidates
        .iter()
        .enumerate()
        .filter_map(|(idx, cand)| {
            let (ver, src) = get_ver_src(cand);
            (req_ver.is_none_or(|v| ver == v) && req_src.is_none_or(|s| src == Some(s)))
                .then_some(idx)
        })
        .collect();
    match matches.len() {
        0 => {
            if fail_on_no_match && !candidates.is_empty() {
                Err(format!(
                    "Unresolved dependency specification '{}' in Cargo.lock (0 of {} candidates matched version={:?} source={:?})",
                    dep_label,
                    candidates.len(),
                    req_ver,
                    req_src
                ))
            } else {
                Ok(None)
            }
        }
        1 => Ok(Some(matches[0])),
        _ => {
            if req_ver.is_some() && req_src.is_none() {
                let local_only: Vec<usize> = matches
                    .iter()
                    .copied()
                    .filter(|&i| get_ver_src(&candidates[i]).1.is_none())
                    .collect();
                if local_only.len() == 1 {
                    return Ok(Some(local_only[0]));
                }
            } else if req_ver.is_none()
                && req_src.is_none()
                && matches.iter().all(|&i| {
                    get_ver_src(&candidates[i]).0 == get_ver_src(&candidates[matches[0]]).0
                })
            {
                if let Some(ps) = parent_source {
                    let same_src: Vec<usize> = matches
                        .iter()
                        .copied()
                        .filter(|&i| get_ver_src(&candidates[i]).1 == Some(ps))
                        .collect();
                    if same_src.len() == 1 {
                        return Ok(Some(same_src[0]));
                    }
                }
            }
            Err(format!(
                "Ambiguous dependency specification '{}' in Cargo.lock ({} matching packages found; specify version and source)",
                dep_label,
                matches.len()
            ))
        }
    }
}

/// Checks if a package manifest has an active dependency on `dep_name`,
/// filtering out inactive optional dependencies, foreign target cfgs, and dev-dependencies for non-test units.
pub fn has_active_dependency(
    pkg_toml: &CargoToml,
    dep_name: &str,
    unit_features: &[String],
    is_test: bool,
    has_build_script: bool,
    explicit_target: Option<&str>,
    start_dir: &Path,
) -> Result<bool, RustAdapterError> {
    let mut cache = TargetCfgCache::new();
    has_active_dependency_cached(
        pkg_toml,
        dep_name,
        unit_features,
        is_test,
        has_build_script,
        explicit_target,
        start_dir,
        &mut cache,
    )
}

/// Memoized variant of [`has_active_dependency`] reusing a [`TargetCfgCache`] across units.
#[allow(clippy::too_many_arguments)]
pub fn has_active_dependency_cached(
    pkg_toml: &CargoToml,
    dep_name: &str,
    unit_features: &[String],
    is_test: bool,
    has_build_script: bool,
    explicit_target: Option<&str>,
    start_dir: &Path,
    cfg_cache: &mut TargetCfgCache,
) -> Result<bool, RustAdapterError> {
    let clean = dep_name.replace('-', "_");
    let check_dep = |alias: &str, d: &TomlDependency| -> bool {
        let (real_name, has_custom_pkg) = match d {
            TomlDependency::Simple(_) => (alias, false),
            TomlDependency::Detailed(dt) => (
                dt.package_name.as_deref().unwrap_or(alias),
                dt.package_name.is_some(),
            ),
        };
        let name_matches = if has_custom_pkg {
            real_name == dep_name || real_name.replace('-', "_") == clean
        } else {
            alias == dep_name
                || alias.replace('-', "_") == clean
                || real_name == dep_name
                || real_name.replace('-', "_") == clean
        };
        if name_matches {
            let is_optional = match d {
                TomlDependency::Simple(_) => false,
                TomlDependency::Detailed(dt) => dt.optional.unwrap_or(false),
            };
            if !is_optional {
                return true;
            }
            if unit_features
                .iter()
                .any(|f| f == alias || f.replace('-', "_") == clean || f == real_name)
            {
                return true;
            }
            for f in unit_features {
                if let Some(sub_feats) = pkg_toml.features.get(f) {
                    for sub in sub_feats {
                        let sub_dep = sub.strip_prefix("dep:").unwrap_or(sub);
                        let sub_crate = sub_dep.split('/').next().unwrap_or(sub_dep);
                        if sub_crate.ends_with('?') {
                            continue;
                        }
                        let sub_clean = sub_crate.replace('-', "_");
                        if sub_crate == alias || sub_clean == clean || sub_crate == real_name {
                            return true;
                        }
                    }
                }
            }
            return false;
        }
        false
    };

    for (alias, d) in &pkg_toml.dependencies {
        if check_dep(alias, d) {
            return Ok(true);
        }
    }

    if has_build_script {
        for (alias, d) in &pkg_toml.build_dependencies {
            if check_dep(alias, d) {
                return Ok(true);
            }
        }
    }

    for (target_spec, target) in &pkg_toml.target {
        let is_target_incompat =
            eval_target_cfg_incompat(target_spec, false, explicit_target, start_dir, cfg_cache)?;
        let is_host_incompat =
            eval_target_cfg_incompat(target_spec, true, explicit_target, start_dir, cfg_cache)?;
        if !is_target_incompat {
            for (alias, d) in &target.dependencies {
                if check_dep(alias, d) {
                    return Ok(true);
                }
            }
        }
        if has_build_script && !is_host_incompat {
            for (alias, d) in &target.build_dependencies {
                if check_dep(alias, d) {
                    return Ok(true);
                }
            }
        }
        if is_test && !is_target_incompat {
            for (alias, d) in &target.dev_dependencies {
                if check_dep(alias, d) {
                    return Ok(true);
                }
            }
        }
    }

    if is_test {
        for (alias, d) in &pkg_toml.dev_dependencies {
            if check_dep(alias, d) {
                return Ok(true);
            }
        }
    }

    Ok(false)
}

/// Checks if a package manifest declares `dep_name` in any dependency section:
/// `[dependencies]`, `[build-dependencies]`, or `[target.*.dependencies]`
/// (or `[dev-dependencies]` if `is_test` is true), failing closed on cfg discovery errors.
pub fn is_declared_dependency(
    pkg_toml: &CargoToml,
    dep_name: &str,
    is_test: bool,
    explicit_target: Option<&str>,
    start_dir: &Path,
) -> Result<bool, RustAdapterError> {
    let mut cache = TargetCfgCache::new();
    is_declared_dependency_cached(
        pkg_toml,
        dep_name,
        is_test,
        explicit_target,
        start_dir,
        &mut cache,
    )
}

/// Memoized variant of [`is_declared_dependency`] reusing a [`TargetCfgCache`] across packages.
pub fn is_declared_dependency_cached(
    pkg_toml: &CargoToml,
    dep_name: &str,
    is_test: bool,
    explicit_target: Option<&str>,
    start_dir: &Path,
    cfg_cache: &mut TargetCfgCache,
) -> Result<bool, RustAdapterError> {
    let clean = dep_name.replace('-', "_");
    let check_dep = |alias: &str, d: &TomlDependency| -> bool {
        let (real_name, has_custom_pkg) = match d {
            TomlDependency::Simple(_) => (alias, false),
            TomlDependency::Detailed(dt) => (
                dt.package_name.as_deref().unwrap_or(alias),
                dt.package_name.is_some(),
            ),
        };
        if has_custom_pkg {
            real_name == dep_name || real_name.replace('-', "_") == clean
        } else {
            alias == dep_name
                || alias.replace('-', "_") == clean
                || real_name == dep_name
                || real_name.replace('-', "_") == clean
        }
    };

    for (alias, d) in &pkg_toml.dependencies {
        if check_dep(alias, d) {
            return Ok(true);
        }
    }

    let build_disabled = matches!(
        pkg_toml.package.as_ref().and_then(|p| p.build.as_ref()),
        Some(TomlBuild::Disabled(false))
    );

    if !build_disabled {
        for (alias, d) in &pkg_toml.build_dependencies {
            if check_dep(alias, d) {
                return Ok(true);
            }
        }
    }

    for (target_spec, target) in &pkg_toml.target {
        let is_target_incompat =
            eval_target_cfg_incompat(target_spec, false, explicit_target, start_dir, cfg_cache)?;
        let is_host_incompat =
            eval_target_cfg_incompat(target_spec, true, explicit_target, start_dir, cfg_cache)?;
        if !is_target_incompat {
            for (alias, d) in &target.dependencies {
                if check_dep(alias, d) {
                    return Ok(true);
                }
            }
        }
        if !build_disabled && !is_host_incompat {
            for (alias, d) in &target.build_dependencies {
                if check_dep(alias, d) {
                    return Ok(true);
                }
            }
        }
        if is_test && !is_target_incompat {
            for (alias, d) in &target.dev_dependencies {
                if check_dep(alias, d) {
                    return Ok(true);
                }
            }
        }
    }

    if is_test {
        for (alias, d) in &pkg_toml.dev_dependencies {
            if check_dep(alias, d) {
                return Ok(true);
            }
        }
    }

    Ok(false)
}
