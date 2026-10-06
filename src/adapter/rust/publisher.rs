/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::types::EffectivePlanExplanation;
use crate::core::action::{ActionKind, BuildPlan};
use crate::core::action_cache::{ActionCache, ActionCacheEntry, CachedOutput};
use crate::core::cas::ContentAddressedStore;
use anyhow::Result;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

#[allow(dead_code)]
pub(crate) fn publish_built_artifacts(
    target_dir: &Path,
    profile: &str,
    plan: &BuildPlan,
    explanation: &EffectivePlanExplanation,
    unit_keys: &HashMap<String, String>,
    cas: &ContentAddressedStore,
    ac: &ActionCache,
) -> Result<()> {
    for action in &plan.actions {
        let matching_unit = explanation.units.iter().find(|u| u.unit_id() == action.id);

        if let Some(unit) = matching_unit {
            let u_id = unit.unit_id();
            if let Some(action_key) = unit_keys.get(&u_id) {
                if action.outputs.is_empty() {
                    continue;
                }

                let mut stored_outputs = Vec::new();
                let mut all_outputs_stored = true;

                for output_file in &action.outputs {
                    if !output_file.is_file() {
                        all_outputs_stored = false;
                        break;
                    }

                    match cas.store_file(output_file) {
                        Ok((cas_hash, file_size)) => {
                            let rel = output_file
                                .strip_prefix(target_dir)
                                .unwrap_or(output_file)
                                .to_path_buf();
                            let is_executable = (action.kind == ActionKind::CompileBin
                                || action.kind == ActionKind::BuildScript)
                                && (output_file.extension().is_none()
                                    || output_file
                                        .extension()
                                        .map(|e| e == "exe")
                                        .unwrap_or(false));
                            if !stored_outputs
                                .iter()
                                .any(|c: &CachedOutput| c.relative_path == rel)
                            {
                                stored_outputs.push(CachedOutput {
                                    relative_path: rel,
                                    cas_hash,
                                    file_size,
                                    executable: is_executable,
                                });
                            }
                        }
                        Err(_) => {
                            all_outputs_stored = false;
                            break;
                        }
                    }
                }

                // Also capture .fingerprint files for this package if present
                let pkg_name_clean = &unit.package_name;
                let pkg_name_norm = pkg_name_clean.replace('-', "_");
                let fp_dir = target_dir.join(profile).join(".fingerprint");
                if fp_dir.is_dir() {
                    if let Ok(entries) = fs::read_dir(&fp_dir) {
                        for entry in entries.flatten() {
                            let path = entry.path();
                            let fname = entry.file_name();
                            let fname_str = fname.to_string_lossy();
                            if fname_str.starts_with(&format!("{}-", pkg_name_clean))
                                || fname_str.starts_with(&format!("{}-", pkg_name_norm))
                            {
                                if path.is_dir() {
                                    if let Ok(sub_entries) = fs::read_dir(&path) {
                                        for sub in sub_entries.flatten() {
                                            let sub_path = sub.path();
                                            if sub_path.is_file() {
                                                if let Ok((cas_hash, file_size)) =
                                                    cas.store_file(&sub_path)
                                                {
                                                    let rel = sub_path
                                                        .strip_prefix(target_dir)
                                                        .unwrap_or(&sub_path)
                                                        .to_path_buf();
                                                    if !stored_outputs.iter().any(
                                                        |c: &CachedOutput| c.relative_path == rel,
                                                    ) {
                                                        stored_outputs.push(CachedOutput {
                                                            relative_path: rel,
                                                            cas_hash,
                                                            file_size,
                                                            executable: false,
                                                        });
                                                    }
                                                }
                                            }
                                        }
                                    }
                                } else if path.is_file() {
                                    if let Ok((cas_hash, file_size)) = cas.store_file(&path) {
                                        let rel = path
                                            .strip_prefix(target_dir)
                                            .unwrap_or(&path)
                                            .to_path_buf();
                                        if !stored_outputs
                                            .iter()
                                            .any(|c: &CachedOutput| c.relative_path == rel)
                                        {
                                            stored_outputs.push(CachedOutput {
                                                relative_path: rel,
                                                cas_hash,
                                                file_size,
                                                executable: false,
                                            });
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                // Publish ONLY if EVERY required output was captured and stored successfully
                if all_outputs_stored && !stored_outputs.is_empty() {
                    let entry = ActionCacheEntry {
                        action_key: action_key.clone(),
                        action_id: unit.unit_id(),
                        created_at_secs: std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_secs())
                            .unwrap_or(0),
                        outputs: stored_outputs,
                        stdout: String::new(),
                        stderr: String::new(),
                        exit_code: 0,
                    };
                    let _ = ac.put(&entry);
                }
            }
        }
    }
    Ok(())
}
