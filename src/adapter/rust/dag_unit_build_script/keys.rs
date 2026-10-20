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

    if let Some(d) = current_directives {
        let out_dir_to_scan = d
            .out_dir
            .as_deref()
            .or_else(|| staging_dir_holder.map(|p| p.as_path()))
            .unwrap_or_else(|| Path::new(""));
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
                eprintln!(
                    "[mitts::purity] Unit '{}' marked impure: {reason}",
                    unit.unit_id()
                );
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
