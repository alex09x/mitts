/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::build_script::BuildScriptDirectives;
use super::manifest::CargoToml;
use super::types::{BuildProgressEvent, PlannedCompilationUnit};
use crate::core::action::{Action, ActionKind};
use anyhow::Result;
use std::fs;
use std::path::{Path, PathBuf};

pub use super::dag_unit::context::*;

/// Checks the Action Cache and restores binary or dependency rlibs from CAS if valid.
/// Returns Ok(true) if restored from cache, Ok(false) otherwise.
pub fn try_restore_from_cache(
    unit: &PlannedCompilationUnit,
    action_key: &str,
    ctx: &ExecutionContext,
    is_bin: bool,
) -> Result<bool> {
    try_restore_from_cache_with_directives(unit, action_key, ctx, is_bin, None)
}

/// Checks the Action Cache using explicitly supplied build script directives for the current run.
pub fn try_restore_from_cache_with_directives(
    unit: &PlannedCompilationUnit,
    action_key: &str,
    ctx: &ExecutionContext,
    is_bin: bool,
    current_directives: Option<&BuildScriptDirectives>,
) -> Result<bool> {
    try_restore_from_cache_with_directives_and_purity(
        unit,
        action_key,
        ctx,
        is_bin,
        current_directives,
        None,
    )
}

/// Checks the Action Cache using explicitly supplied build script directives and precomputed purity for the current run.
pub fn try_restore_from_cache_with_directives_and_purity(
    unit: &PlannedCompilationUnit,
    action_key: &str,
    ctx: &ExecutionContext,
    is_bin: bool,
    current_directives: Option<&BuildScriptDirectives>,
    purity: Option<&super::build_script_purity::BuildScriptPurity>,
) -> Result<bool> {
    if !ctx.is_cache_enabled || action_key.contains(":unhashable:") {
        return Ok(false);
    }
    let is_mutable = super::unit_inputs::is_mutable_unit(unit);
    let ws_root = if is_mutable {
        Some(super::compiler::find_workspace_root(&unit.manifest_path))
    } else {
        None
    };
    let current_ws_hash = ws_root
        .as_ref()
        .map(|r| crate::core::hash::Sha256::digest(r.to_string_lossy().as_bytes()));

    let mut candidate_keys = Vec::new();
    if is_mutable {
        if let Ok(Some(manifest)) = ctx.ac.get_manifest(action_key) {
            let has_build = super::unit_env::unit_has_build_script(unit);
            if has_build && current_directives.is_none() {
                return Ok(false);
            }
            let mut unit_env = super::unit_env::compute_unit_rustc_env_with_target(
                unit,
                current_directives,
                None,
                Some(ctx.target_dir),
                ctx.opt.release,
            );
            let out_dir = current_directives.and_then(|d| d.out_dir.as_deref());
            if has_build {
                let purity_hash = purity.map(|p| p.out_dir_hash.as_str());
                if let Some(h) = purity_hash {
                    unit_env.insert("OUT_DIR".to_string(), h.to_string());
                } else if let (Some(d), Some(out)) = (current_directives, out_dir) {
                    let pkg_dir = unit.manifest_path.parent().unwrap_or(Path::new("."));
                    let empty_refs = std::collections::HashSet::new();
                    if let Ok(p) =
                        super::build_script_purity::assess_build_script_purity_with_dep_info(
                            pkg_dir,
                            ws_root.as_deref(),
                            out,
                            Some(ctx.target_dir),
                            Some(ctx.cas.root()),
                            d,
                            Some(&empty_refs),
                        )
                    {
                        unit_env.insert("OUT_DIR".to_string(), p.out_dir_hash);
                    }
                }
            }
            if let Some(matched_res_key) =
                manifest.find_matching_entry(ws_root.as_deref(), Some(&unit_env), out_dir)
            {
                if let Some(matched_entry) = manifest
                    .entries
                    .iter()
                    .find(|e| e.result_key == matched_res_key)
                {
                    super::dag_unit_publish::record_extra_inputs_in_ctx(
                        ctx,
                        ws_root.as_deref(),
                        &matched_entry.extra_inputs,
                    );
                }
                candidate_keys.push(matched_res_key.clone());
                if let Some(ref h) = current_ws_hash {
                    candidate_keys.push(format!("{}:ws_leak:{}", matched_res_key, h));
                }
            }
        }
    } else {
        candidate_keys.push(action_key.to_string());
    }

    if candidate_keys.is_empty() {
        return Ok(false);
    }

    let mut matched_entry = None;
    let mut effective_key = action_key.to_string();
    for cand in candidate_keys {
        if let Ok(Some(entry)) = ctx.ac.get(&cand) {
            matched_entry = Some(entry);
            effective_key = cand;
            break;
        }
    }

    let entry = match matched_entry {
        Some(e) => e,
        None => return Ok(false),
    };
    let res_key = effective_key
        .split(":ws_leak:")
        .next()
        .unwrap_or(&effective_key)
        .to_string();
    let artifact_key = super::dag_unit_publish::artifact_disambiguator(&res_key, None);

    let needs_linkable_lib = !is_bin
        && (unit.for_host
            || !matches!(
                ctx.opt.mode,
                crate::adapter::BuildMode::Check | crate::adapter::BuildMode::Clippy
            ));
    let mut valid = !entry.outputs.is_empty()
        && (!needs_linkable_lib
            || entry.outputs.iter().any(|o| {
                matches!(
                    o.relative_path.extension().and_then(|e| e.to_str()),
                    Some("rlib" | "dylib" | "so")
                )
            }));
    for out in &entry.outputs {
        if fs::metadata(ctx.cas.object_path(&out.cas_hash))
            .map(|m| m.len() != out.file_size)
            .unwrap_or(true)
        {
            valid = false;
            break;
        }
    }

    let safe_eff = crate::core::action_cache::safe_key_file_name(&effective_key);
    let meta_dir = ctx.cas.root().join("metadata");
    let eff_meta = meta_dir.join(format!("{safe_eff}.toml"));
    let meta_path = if eff_meta.is_file() {
        eff_meta
    } else {
        let safe_act = crate::core::action_cache::safe_key_file_name(action_key);
        meta_dir.join(format!("{safe_act}.toml"))
    };
    let pkg_toml = CargoToml::from_file(&unit.manifest_path).unwrap_or_default();
    let has_links = pkg_toml.package.as_ref().is_some_and(|p| p.links.is_some());
    if has_links && !meta_path.is_file() {
        return Ok(false);
    }

    if valid {
        if is_bin {
            let profile_name = if ctx.opt.release { "release" } else { "debug" };
            let dest_bin =
                super::dag_unit_publish::unit_dest_binary(unit, ctx.target_dir, profile_name);
            for out in &entry.outputs {
                if out.executable {
                    let rel_dest = ctx.target_dir.join(&out.relative_path);
                    let rel_key = out.relative_path.to_string_lossy();
                    if crate::core::target_index::check_or_restore(
                        ctx.target_index,
                        ctx.cas,
                        &out.cas_hash,
                        &rel_dest,
                        &rel_key,
                        true,
                    )
                    .is_err()
                    {
                        return Ok(false);
                    }

                    if rel_dest != dest_bin {
                        let _ = crate::core::link_intra_workspace(
                            &rel_dest,
                            &dest_bin,
                            &[ctx.target_dir],
                            true,
                        );
                    }
                    let clean = unit.target_name.replace('-', "_");
                    let alt_path = dest_bin.parent().unwrap_or(&dest_bin).join(&clean);
                    if alt_path != dest_bin {
                        let _ = crate::core::link_intra_workspace(
                            &dest_bin,
                            &alt_path,
                            &[ctx.target_dir],
                            true,
                        );
                    }
                    break;
                }
            }
        } else {
            let mut staged_completed: Vec<(String, PathBuf, bool)> = Vec::new();
            for out in &entry.outputs {
                let ext = out
                    .relative_path
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("rlib");
                if !ctx.opt.native || unit.is_workspace {
                    let profile_name = if ctx.opt.release { "release" } else { "debug" };
                    let deps_dir = ctx.target_dir.join(profile_name).join("deps");
                    let _ = fs::create_dir_all(&deps_dir);
                    let target_dep_dest = deps_dir.join(&out.relative_path);
                    let rel_key = format!("{}/deps/{}", profile_name, out.relative_path.display());
                    if crate::core::target_index::check_or_restore(
                        ctx.target_index,
                        ctx.cas,
                        &out.cas_hash,
                        &target_dep_dest,
                        &rel_key,
                        false,
                    )
                    .is_err()
                    {
                        return Ok(false);
                    }
                    let clean = unit.target_name.replace('-', "_");
                    let unhashed = deps_dir.join(format!("lib{clean}.{ext}"));
                    if unhashed != target_dep_dest {
                        let _ = crate::core::link_intra_workspace(
                            &target_dep_dest,
                            &unhashed,
                            &[ctx.target_dir],
                            false,
                        );
                    }
                }

                if matches!(ext, "rlib" | "dylib" | "so" | "rmeta") {
                    let is_meta_only = ext == "rmeta"
                        && !entry.outputs.iter().any(|o| {
                            matches!(
                                o.relative_path.extension().and_then(|e| e.to_str()),
                                Some("rlib" | "dylib" | "so")
                            )
                        });
                    let disambig = if is_meta_only {
                        format!("{artifact_key}-m")
                    } else {
                        artifact_key.clone()
                    };
                    let Ok(named) = ctx.cas.named_artifact_path_with_disambiguator(
                        &unit.target_name,
                        &out.cas_hash,
                        &disambig,
                        ext,
                    ) else {
                        return Ok(false);
                    };
                    staged_completed.push((unit.unit_id(), named, ext != "rmeta"));
                }
            }
            let mut map = ctx.completed_artifacts.lock().unwrap();
            for (uid, named, overwrite) in staged_completed {
                if overwrite {
                    map.insert(uid, named);
                } else {
                    map.entry(uid).or_insert(named);
                }
            }
        }

        if let Some(rk_map) = ctx.resolved_keys {
            rk_map.lock().unwrap().insert(unit.unit_id(), res_key);
        }

        let effective_dirs: Option<BuildScriptDirectives> = if let Some(d) = current_directives {
            Some(d.clone())
        } else if meta_path.is_file() {
            fs::read_to_string(&meta_path)
                .ok()
                .and_then(|c| toml::from_str::<BuildScriptDirectives>(&c).ok())
        } else {
            None
        };
        if let Some(ref d) = effective_dirs {
            let dep_vars = d.dep_env_vars();
            if !dep_vars.is_empty() {
                let mut all_meta = ctx.dep_build_metadata.lock().unwrap();
                all_meta.insert(unit.unit_id(), dep_vars.clone());
                all_meta.insert(unit.package_name.clone(), dep_vars);
            }
            super::dag_unit_build_script::record_completed_directives(
                &unit.unit_id(),
                d,
                ctx.completed_cfgs,
                ctx.completed_env,
                ctx.completed_link_args,
            );
        }

        for msg in [&entry.stdout, &entry.stderr] {
            if !msg.is_empty() {
                ctx.compiler_messages.lock().unwrap().push(msg.clone());
                if let Some(ref cb) = ctx.opt.progress {
                    cb.0(BuildProgressEvent::CompilerMessage(msg.clone()));
                }
            }
        }

        let kind = if is_bin {
            ActionKind::CompileBin
        } else {
            ActionKind::CompileRlib
        };
        let env = super::unit_env::compute_unit_rustc_env_with_target(
            unit,
            effective_dirs.as_ref(),
            None,
            Some(ctx.target_dir),
            ctx.opt.release,
        );
        let outputs = entry
            .outputs
            .iter()
            .map(|o| ctx.target_dir.join(&o.relative_path))
            .collect();
        ctx.recorded_actions.lock().unwrap().push(Action {
            id: entry.action_id.clone(),
            kind,
            description: format!("Restored {} from CAS", entry.action_id),
            toolchain: ctx.toolchain.clone(),
            inputs: Vec::new(),
            outputs,
            args: Vec::new(),
            env,
            dependencies: Vec::new(),
        });
        if let Some(ref cb) = ctx.opt.progress {
            cb.0(BuildProgressEvent::UnitFinished);
        }
        if let Some(c) = ctx.cached_actions {
            c.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        return Ok(true);
    }
    Ok(false)
}
