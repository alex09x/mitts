/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::adapter::rust::build_script_directives::BuildScriptDirectives;
use crate::adapter::rust::build_script_purity::BuildScriptPurity;
use crate::adapter::rust::dag_cache::ExecutionContext;
use crate::adapter::rust::types::PlannedCompilationUnit;
use std::fs;
use std::path::{Path, PathBuf};

pub(crate) struct PreCompileKeyInfo {
    pub effective_action_key: String,
    pub ws_fallback_key: Option<String>,
    pub build_purity_opt: Option<BuildScriptPurity>,
}

pub(crate) fn prepare_unit_build_script_keys(
    unit: &PlannedCompilationUnit,
    action_key: &str,
    ctx: &ExecutionContext,
    current_directives: Option<&BuildScriptDirectives>,
    staging_dir_holder: Option<&PathBuf>,
) -> PreCompileKeyInfo {
    let mut build_purity_opt = None;
    let ws_root = match unit.is_workspace {
        true => crate::adapter::rust::compiler::find_workspace_root(&unit.manifest_path),
        false => ctx
            .target_dir
            .parent()
            .unwrap_or(ctx.target_dir)
            .to_path_buf(),
    };

    if let Some((d, out_dir_to_scan)) = current_directives.and_then(|directives| {
        directives
            .out_dir
            .as_deref()
            .or_else(|| staging_dir_holder.map(|path| path.as_path()))
            .map(|out| (directives, out))
    }) {
        let pkg_dir = unit.manifest_path.parent().unwrap_or(Path::new("."));
        let empty_refs = std::collections::HashSet::new();
        if let Ok(purity) =
            crate::adapter::rust::build_script_purity::assess_build_script_purity_with_dep_info(
                pkg_dir,
                Some(&ws_root),
                out_dir_to_scan,
                Some(ctx.target_dir),
                Some(ctx.cas.root()),
                d,
                Some(&empty_refs),
            )
        {
            build_purity_opt = Some(purity);
        }
    }

    if let Some(d) = current_directives {
        let bd = d.compute_compile_hash();
        let ws_hash = crate::core::hash::Sha256::digest(ws_root.to_string_lossy().as_bytes());
        let ws_key = format!("{action_key}:bd:{bd}:ws:{ws_hash}");
        if let Some(ref purity) = build_purity_opt {
            if purity.is_pure {
                PreCompileKeyInfo {
                    effective_action_key: format!(
                        "{action_key}:bd:{bd}:out:{}",
                        purity.out_dir_hash
                    ),
                    ws_fallback_key: Some(ws_key),
                    build_purity_opt,
                }
            } else {
                let reason = purity.reason.as_deref().unwrap_or("unknown");
                if crate::core::verbosity::level() >= 1 {
                    eprintln!(
                        "[mitts::purity] Unit '{}' marked impure: {reason}",
                        unit.unit_id()
                    );
                }
                PreCompileKeyInfo {
                    effective_action_key: ws_key,
                    ws_fallback_key: None,
                    build_purity_opt,
                }
            }
        } else {
            PreCompileKeyInfo {
                effective_action_key: format!("{action_key}:bd:{bd}"),
                ws_fallback_key: None,
                build_purity_opt,
            }
        }
    } else {
        PreCompileKeyInfo {
            effective_action_key: action_key.to_string(),
            ws_fallback_key: None,
            build_purity_opt,
        }
    }
}

pub(crate) fn finalize_post_rustc_purity(
    unit: &PlannedCompilationUnit,
    clean_crate: &str,
    ctx: &ExecutionContext,
    current_directives: Option<&BuildScriptDirectives>,
    staging_dir: &Path,
    pre_info: &PreCompileKeyInfo,
) -> (
    String,
    Option<super::super::build_script_purity::BuildScriptPurity>,
) {
    if let (Some(d), Some(pre_purity)) = (current_directives, pre_info.build_purity_opt.as_ref()) {
        if pre_purity.is_pure {
            let pkg_dir = unit.manifest_path.parent().unwrap_or(Path::new("."));
            let ws_root = match unit.is_workspace {
                true => super::super::compiler::find_workspace_root(&unit.manifest_path),
                false => ctx
                    .target_dir
                    .parent()
                    .unwrap_or(ctx.target_dir)
                    .to_path_buf(),
            };
            let out_dir_to_scan = d.out_dir.as_deref().unwrap_or(staging_dir);
            let ref_files = super::super::build_script_purity::extract_out_dir_files_from_dep_info(
                staging_dir,
                out_dir_to_scan,
                clean_crate,
                Some(&ws_root),
            );
            if let Ok(post_purity) =
                super::super::build_script_purity::assess_build_script_purity_with_dep_info(
                    pkg_dir,
                    Some(&ws_root),
                    out_dir_to_scan,
                    Some(ctx.target_dir),
                    Some(ctx.cas.root()),
                    d,
                    ref_files.as_ref(),
                )
            {
                if !post_purity.is_pure {
                    let reason = post_purity.reason.as_deref().unwrap_or("unknown");
                    if crate::core::verbosity::level() >= 1 {
                        eprintln!(
                            "[mitts::purity] Unit '{}' marked impure: {reason}",
                            unit.unit_id()
                        );
                    }
                    let fb_key = pre_info
                        .ws_fallback_key
                        .clone()
                        .unwrap_or_else(|| pre_info.effective_action_key.clone());
                    if let Ok(toml_str) = toml::to_string(d) {
                        let meta_dir = ctx.cas.root().join("metadata");
                        let safe_meta_key = crate::core::action_cache::safe_key_file_name(&fb_key);
                        let _ = fs::write(meta_dir.join(format!("{safe_meta_key}.toml")), toml_str);
                    }
                    return (fb_key, Some(post_purity));
                }
                return (pre_info.effective_action_key.clone(), Some(post_purity));
            }
        }
    }
    (
        pre_info.effective_action_key.clone(),
        pre_info.build_purity_opt.clone(),
    )
}

pub(super) fn register_directives_in_ctx(
    unit: &PlannedCompilationUnit,
    directives: &BuildScriptDirectives,
    ctx: &ExecutionContext,
) {
    let dep_vars = directives.dep_env_vars();
    if !dep_vars.is_empty() {
        let mut all_meta = ctx.dep_build_metadata.lock().unwrap();
        all_meta.insert(unit.unit_id(), dep_vars.clone());
        all_meta.insert(unit.package_name.clone(), dep_vars);
    }

    let pkg_dir = unit.manifest_path.parent().unwrap_or(Path::new("."));
    if let Some(env_map) = ctx.tracked_env_vars {
        let mut guard = env_map.lock().unwrap();
        for var in &directives.rerun_if_env_changed {
            guard.insert(var.clone(), std::env::var(var).ok());
        }
    }
    if let Some(files_map) = ctx.tracked_extra_files {
        let mut guard = files_map.lock().unwrap();
        for (file_rel, expected_hash) in &directives.rerun_if_changed_hashes {
            let fpath = if Path::new(file_rel).is_absolute() {
                PathBuf::from(file_rel)
            } else {
                pkg_dir.join(file_rel)
            };
            guard.insert(fpath, expected_hash.clone());
        }
    }
}
