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
use super::types::{BuildProgressEvent, PlannedCompilationUnit, RustBuildOptions};
use crate::core::action::{Action, ActionKind, ToolchainInfo};
use crate::core::action_cache::ActionCache;
use crate::core::cas::ContentAddressedStore;
use anyhow::Result;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub type MetadataReadyCallback<'a> = &'a (dyn Fn(&str, &Path) + Send + Sync);
pub type CompletedCfgsMap = Arc<Mutex<HashMap<String, Vec<String>>>>;
pub type CompletedEnvMap = Arc<Mutex<HashMap<String, HashMap<String, String>>>>;
pub type CompletedLinkArgsMap = Arc<Mutex<HashMap<String, Vec<String>>>>;
pub type TrackedExtraFilesMap = Arc<Mutex<std::collections::BTreeMap<PathBuf, String>>>;
pub type TrackedEnvVarsMap = Arc<Mutex<std::collections::BTreeMap<String, Option<String>>>>;

pub struct ExecutionContext<'a> {
    pub opt: &'a RustBuildOptions,
    pub toolchain: &'a ToolchainInfo,
    pub target_dir: &'a Path,
    pub cas: &'a ContentAddressedStore,
    pub ac: &'a ActionCache,
    pub is_cache_enabled: bool,
    pub completed_artifacts: &'a Arc<Mutex<HashMap<String, PathBuf>>>,
    pub recorded_actions: &'a Arc<Mutex<Vec<Action>>>,
    pub compiler_messages: &'a Arc<Mutex<Vec<String>>>,
    pub any_missed: &'a Arc<Mutex<bool>>,
    pub dep_build_metadata: &'a Arc<Mutex<HashMap<String, HashMap<String, String>>>>,
    pub resolved_keys: Option<&'a Arc<Mutex<HashMap<String, String>>>>,
    pub jobserver: Option<&'a Arc<crate::core::JobserverPool>>,
    pub is_first_job: Option<&'a Arc<std::sync::atomic::AtomicBool>>,
    pub on_metadata_ready: Option<MetadataReadyCallback<'a>>,
    pub compiled_actions: Option<&'a Arc<std::sync::atomic::AtomicUsize>>,
    pub cached_actions: Option<&'a Arc<std::sync::atomic::AtomicUsize>>,
    pub completed_cfgs: Option<&'a CompletedCfgsMap>,
    pub completed_env: Option<&'a CompletedEnvMap>,
    pub completed_link_args: Option<&'a CompletedLinkArgsMap>,
    pub target_index: Option<&'a Arc<Mutex<crate::core::TargetArtifactIndex>>>,
    pub tracked_extra_files: Option<&'a TrackedExtraFilesMap>,
    pub tracked_env_vars: Option<&'a TrackedEnvVarsMap>,
}

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

    // 1. Resolve candidate action keys (manifest match or direct key)
    let mut candidate_keys = Vec::new();
    if is_mutable {
        if let Ok(Some(manifest)) = ctx.ac.get_manifest(action_key) {
            let has_build = super::unit_env::unit_has_build_script(unit);
            if has_build && current_directives.is_none() {
                // Fail-closed: unit with build.rs cannot be restored from manifest without current run directives
                return Ok(false);
            }
            let mut unit_env =
                super::unit_env::compute_unit_rustc_env(unit, current_directives, None);
            let out_dir = current_directives.and_then(|d| d.out_dir.as_deref());
            if has_build {
                if let (Some(d), Some(out)) = (current_directives, out_dir) {
                    let pkg_dir = unit.manifest_path.parent().unwrap_or(Path::new("."));
                    let empty_refs = std::collections::HashSet::new();
                    if let Ok(purity) =
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
                        unit_env.insert("OUT_DIR".to_string(), purity.out_dir_hash);
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

    let mut valid = !entry.outputs.is_empty();
    for out in &entry.outputs {
        let obj_path = ctx.cas.object_path(&out.cas_hash);
        if let Ok(meta) = fs::metadata(&obj_path) {
            if meta.len() != out.file_size {
                valid = false;
                break;
            }
        } else {
            valid = false;
            break;
        }
    }

    let safe_eff = crate::core::action_cache::safe_key_file_name(&effective_key);
    let meta_path = ctx
        .cas
        .root()
        .join("metadata")
        .join(format!("{}.toml", safe_eff));
    let meta_path = if meta_path.is_file() {
        meta_path
    } else {
        let safe_act = crate::core::action_cache::safe_key_file_name(action_key);
        ctx.cas
            .root()
            .join("metadata")
            .join(format!("{}.toml", safe_act))
    };
    let pkg_toml = CargoToml::from_file(&unit.manifest_path).unwrap_or_default();
    let has_links = pkg_toml
        .package
        .as_ref()
        .map(|p| p.links.is_some())
        .unwrap_or(false);
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
                    let clean_crate = unit.target_name.replace('-', "_");
                    let alt_path = dest_bin.parent().unwrap_or(&dest_bin).join(&clean_crate);
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
                    let clean_crate = unit.target_name.replace('-', "_");
                    let unhashed = deps_dir.join(format!("lib{}.{}", clean_crate, ext));
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
                    let Ok(named) = ctx.cas.named_artifact_path_with_disambiguator(
                        &unit.target_name,
                        &out.cas_hash,
                        action_key,
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
            let res_key = effective_key
                .split(":ws_leak:")
                .next()
                .unwrap_or(&effective_key);
            rk_map
                .lock()
                .unwrap()
                .insert(unit.unit_id(), res_key.to_string());
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

        ctx.recorded_actions.lock().unwrap().push(Action {
            id: entry.action_id.clone(),
            kind: if is_bin {
                ActionKind::CompileBin
            } else {
                ActionKind::CompileRlib
            },
            description: format!("Restored {} from CAS", entry.action_id),
            toolchain: ctx.toolchain.clone(),
            inputs: Vec::new(),
            outputs: entry
                .outputs
                .iter()
                .map(|o| ctx.target_dir.join(&o.relative_path))
                .collect(),
            args: Vec::new(),
            env: unit
                .manifest_path
                .parent()
                .map(|p| {
                    (
                        "CARGO_MANIFEST_DIR".into(),
                        p.to_string_lossy().into_owned(),
                    )
                })
                .into_iter()
                .chain([
                    ("CARGO_PKG_NAME".into(), unit.package_name.clone()),
                    ("CARGO_PKG_VERSION".into(), unit.version.clone()),
                ])
                .collect(),
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
