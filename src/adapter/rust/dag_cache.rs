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
    if !ctx.is_cache_enabled {
        return Ok(false);
    }

    let ws_root = if unit.is_workspace {
        Some(super::compiler::find_workspace_root(&unit.manifest_path))
    } else {
        None
    };
    let current_ws_hash = ws_root
        .as_ref()
        .map(|r| crate::core::hash::Sha256::digest(r.to_string_lossy().as_bytes()));

    // 1. Resolve candidate action keys (manifest match or direct key)
    let mut candidate_keys = Vec::new();
    if unit.is_workspace {
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
                    if let Ok(purity) = super::build_script_purity::assess_build_script_purity(
                        pkg_dir,
                        ws_root.as_deref(),
                        out,
                        Some(ctx.target_dir),
                        Some(ctx.cas.root()),
                        d,
                    ) {
                        if purity.is_pure {
                            unit_env.insert("OUT_DIR".to_string(), purity.out_dir_hash);
                        }
                    }
                }
            }
            if let Some(matched_res_key) =
                manifest.find_matching_entry(ws_root.as_deref(), Some(&unit_env), out_dir)
            {
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

    let meta_path = ctx
        .cas
        .root()
        .join("metadata")
        .join(format!("{}.toml", effective_key));
    let meta_path = if meta_path.is_file() {
        meta_path
    } else {
        ctx.cas
            .root()
            .join("metadata")
            .join(format!("{}.toml", action_key))
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

        if let Some(d) = current_directives {
            let dep_vars = d.dep_env_vars();
            if !dep_vars.is_empty() {
                let mut all_meta = ctx.dep_build_metadata.lock().unwrap();
                all_meta.insert(unit.unit_id(), dep_vars.clone());
                all_meta.insert(unit.package_name.clone(), dep_vars);
            }
        } else if meta_path.is_file() {
            if let Ok(content) = fs::read_to_string(&meta_path) {
                if let Ok(directives) = toml::from_str::<BuildScriptDirectives>(&content) {
                    let dep_vars = directives.dep_env_vars();
                    if !dep_vars.is_empty() {
                        let mut all_meta = ctx.dep_build_metadata.lock().unwrap();
                        all_meta.insert(unit.unit_id(), dep_vars.clone());
                        all_meta.insert(unit.package_name.clone(), dep_vars);
                    }
                }
            }
        }

        if is_bin {
            let profile_name = if ctx.opt.release { "release" } else { "debug" };
            let dest_bin = ctx.target_dir.join(profile_name).join(&unit.target_name);
            for out in &entry.outputs {
                if out.executable {
                    let rel_dest = ctx.target_dir.join(&out.relative_path);
                    ctx.cas.restore_object(&out.cas_hash, &rel_dest, true)?;
                    if rel_dest != dest_bin {
                        if let Some(parent) = dest_bin.parent() {
                            let _ = fs::create_dir_all(parent);
                        }
                        let _ = crate::core::reflink_or_copy(&rel_dest, &dest_bin, true);
                    }
                    break;
                }
            }
        } else {
            for out in &entry.outputs {
                let ext = out
                    .relative_path
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("rlib");
                if ext == "rlib" || ext == "dylib" || ext == "so" {
                    let named = ctx.cas.named_artifact_path_with_disambiguator(
                        &unit.target_name,
                        &out.cas_hash,
                        action_key,
                        ext,
                    )?;
                    if !ctx.opt.native {
                        let profile_name = if ctx.opt.release { "release" } else { "debug" };
                        let deps_dir = ctx.target_dir.join(profile_name).join("deps");
                        let _ = fs::create_dir_all(&deps_dir);
                        let target_dep_dest = deps_dir.join(&out.relative_path);
                        if !target_dep_dest.is_file() {
                            let _ = ctx
                                .cas
                                .restore_object(&out.cas_hash, &target_dep_dest, false);
                        }
                    }

                    ctx.completed_artifacts
                        .lock()
                        .unwrap()
                        .insert(unit.unit_id(), named);
                } else if ext == "rmeta" {
                    let named = ctx.cas.named_artifact_path_with_disambiguator(
                        &unit.target_name,
                        &out.cas_hash,
                        action_key,
                        "rmeta",
                    )?;
                    let mut map = ctx.completed_artifacts.lock().unwrap();
                    map.entry(unit.unit_id()).or_insert(named);
                }
            }
        }

        if !entry.stderr.is_empty() {
            ctx.compiler_messages
                .lock()
                .unwrap()
                .push(entry.stderr.clone());
            if unit.is_workspace {
                if let Some(ref cb) = ctx.opt.progress {
                    cb.0(BuildProgressEvent::CompilerMessage(entry.stderr.clone()));
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
            env: {
                let mut env = HashMap::new();
                if let Some(parent) = unit.manifest_path.parent() {
                    env.insert(
                        "CARGO_MANIFEST_DIR".to_string(),
                        parent.to_string_lossy().to_string(),
                    );
                }
                env.insert("CARGO_PKG_NAME".to_string(), unit.package_name.clone());
                env.insert("CARGO_PKG_VERSION".to_string(), unit.version.clone());
                env
            },
            dependencies: Vec::new(),
        });

        if let Some(ref cb) = ctx.opt.progress {
            cb.0(BuildProgressEvent::UnitFinished);
        }

        return Ok(true);
    }

    Ok(false)
}
