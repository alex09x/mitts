/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::compiler::RustcOutput;
use super::dag_cache::ExecutionContext;
use super::types::PlannedCompilationUnit;
use crate::core::action_cache::{ActionCacheEntry, CachedOutput};
use crate::core::hash::Sha256;
use anyhow::Result;
use std::fs;
use std::path::{Path, PathBuf};

pub(crate) fn unit_dest_binary(
    unit: &PlannedCompilationUnit,
    target_dir: &Path,
    profile: &str,
) -> PathBuf {
    if unit.kind == "test" || unit.kind == "custom-test" {
        target_dir
            .join(profile)
            .join("deps")
            .join(format!("{}-{}", unit.package_name, unit.target_name))
    } else {
        target_dir.join(profile).join(&unit.target_name)
    }
}

pub(crate) struct PublishArgs<'a> {
    pub unit: &'a PlannedCompilationUnit,
    pub action_key: &'a str,
    pub ctx: &'a ExecutionContext<'a>,
    pub staging_dir: &'a Path,
    pub dest_binary: Option<&'a Path>,
    pub output: &'a RustcOutput,
    pub out_dir_purity: Option<&'a super::build_script_purity::BuildScriptPurity>,
    pub out_dir: Option<&'a Path>,
}

pub(crate) fn publish_unit_artifacts(args: PublishArgs<'_>) -> Result<Vec<PathBuf>> {
    let unit = args.unit;
    let action_key = args.action_key;
    let ctx = args.ctx;
    let clean_crate = unit.target_name.replace('-', "_");
    let mut cached_outputs = Vec::new();
    let mut action_outputs = Vec::new();

    // 1. Store executable binary in CAS from staging first (N3), then materialize in target/
    if let Some(dest) = args.dest_binary {
        if let Some(ref bp) = args.output.primary_output {
            if bp.is_file() {
                let stored_bin = ctx.cas.store_file(bp).ok();
                if let Some(parent) = dest.parent() {
                    let _ = fs::create_dir_all(parent);
                }
                if bp != dest && fs::rename(bp, dest).is_err() {
                    let _ = crate::core::link_intra_workspace(
                        bp,
                        dest,
                        &[args.staging_dir, ctx.target_dir],
                        true,
                    );
                }
                if let Some((bin_hash, bin_size)) = stored_bin {
                    let rel = dest
                        .strip_prefix(ctx.target_dir)
                        .map(PathBuf::from)
                        .unwrap_or_else(|_| {
                            PathBuf::from(if ctx.opt.release { "release" } else { "debug" })
                                .join(&unit.target_name)
                        });
                    let rel_key = rel.to_string_lossy().to_string();
                    if let Some(ti) = ctx.target_index {
                        ti.lock().unwrap().record(rel_key, bin_hash.clone(), dest);
                    }
                    cached_outputs.push(CachedOutput {
                        relative_path: rel,
                        cas_hash: bin_hash,
                        file_size: bin_size,
                        executable: true,
                    });
                }

                let alt_path = dest.parent().unwrap_or(dest).join(&clean_crate);
                if alt_path != *dest {
                    let _ =
                        crate::core::link_intra_workspace(dest, &alt_path, &[ctx.target_dir], true);
                }
                action_outputs.push(dest.to_path_buf());
            }
        }
    } else {
        // 2. Publish primary library output (rlib/dylib/so)
        if let Some(lib_path) = args
            .output
            .primary_output
            .as_ref()
            .or(args.output.metadata_output.as_ref())
        {
            let (hash, size) = ctx.cas.store_file(lib_path)?;
            let ext = lib_path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("rlib");
            let disambig = if ext == "rmeta" {
                format!("{action_key}-m")
            } else {
                action_key.to_string()
            };
            let named = ctx.cas.named_artifact_path_with_disambiguator(
                &unit.target_name,
                &hash,
                &disambig,
                ext,
            )?;

            let file_name = lib_path.file_name().unwrap_or_default();
            cached_outputs.push(CachedOutput {
                relative_path: PathBuf::from(file_name),
                cas_hash: hash.clone(),
                file_size: size,
                executable: false,
            });
            ctx.completed_artifacts
                .lock()
                .unwrap()
                .insert(unit.unit_id(), named.clone());
            action_outputs.push(named);

            if !ctx.opt.native || unit.is_workspace {
                let profile_name = if ctx.opt.release { "release" } else { "debug" };
                let deps_dir = ctx.target_dir.join(profile_name).join("deps");
                let _ = fs::create_dir_all(&deps_dir);
                let dep_copy = deps_dir.join(file_name);
                if lib_path != &dep_copy {
                    let _ = crate::core::link_intra_workspace(
                        lib_path,
                        &dep_copy,
                        &[args.staging_dir, ctx.target_dir],
                        false,
                    );
                }
                if let Some(ti) = ctx.target_index {
                    let rel_key = format!("{}/deps/{}", profile_name, file_name.to_string_lossy());
                    ti.lock().unwrap().record(rel_key, hash.clone(), &dep_copy);
                }
                let unhashed = deps_dir.join(format!("lib{}.{}", clean_crate, ext));
                if unhashed != dep_copy {
                    let _ = crate::core::link_intra_workspace(
                        &dep_copy,
                        &unhashed,
                        &[ctx.target_dir],
                        false,
                    );
                }
            }
        }

        // 3. Publish metadata output (rmeta) if distinct from primary_output
        if let Some(mp) = args
            .output
            .metadata_output
            .as_ref()
            .filter(|mp| args.output.primary_output.as_ref() != Some(*mp))
        {
            let (meta_hash, meta_size) = ctx.cas.store_file(mp)?;
            let _ = ctx.cas.named_artifact_path_with_disambiguator(
                &unit.target_name,
                &meta_hash,
                action_key,
                "rmeta",
            );
            let file_name = mp.file_name().unwrap_or_default();
            cached_outputs.push(CachedOutput {
                relative_path: PathBuf::from(file_name),
                cas_hash: meta_hash.clone(),
                file_size: meta_size,
                executable: false,
            });
            if !ctx.opt.native || unit.is_workspace {
                let profile_name = if ctx.opt.release { "release" } else { "debug" };
                let deps_dir = ctx.target_dir.join(profile_name).join("deps");
                let _ = fs::create_dir_all(&deps_dir);
                let dep_copy = deps_dir.join(file_name);
                if mp != &dep_copy {
                    let _ = crate::core::link_intra_workspace(
                        mp,
                        &dep_copy,
                        &[args.staging_dir, ctx.target_dir],
                        false,
                    );
                }
                if let Some(ti) = ctx.target_index {
                    let rel_key = format!("{}/deps/{}", profile_name, file_name.to_string_lossy());
                    ti.lock()
                        .unwrap()
                        .record(rel_key, meta_hash.clone(), &dep_copy);
                }
                let unhashed = deps_dir.join(format!("lib{}.rmeta", clean_crate));
                if unhashed != dep_copy {
                    let _ = crate::core::link_intra_workspace(
                        &dep_copy,
                        &unhashed,
                        &[ctx.target_dir],
                        false,
                    );
                }
            }
        }
    }

    // 4. Output byte scanning for workspace root leaks (P2)
    let mut contains_ws_root_path = false;
    let is_mutable = super::unit_inputs::is_mutable_unit(unit);
    let ws_root = if is_mutable {
        Some(super::compiler::find_workspace_root(&unit.manifest_path))
    } else {
        None
    };

    if let Some(ref root) = ws_root {
        let ws_needle = root.to_string_lossy().as_bytes().to_vec();
        if !ws_needle.is_empty() {
            let scan_file = |p: &Path| -> bool {
                fs::read(p)
                    .map(|b| {
                        b.windows(ws_needle.len())
                            .any(|w| w == ws_needle.as_slice())
                    })
                    .unwrap_or(false)
            };
            if let Some(bp) = args.dest_binary {
                if scan_file(bp) {
                    contains_ws_root_path = true;
                }
            }
            if let Some(ref lp) = args.output.primary_output {
                if scan_file(lp) {
                    contains_ws_root_path = true;
                }
            }
            if let Some(ref mp) = args.output.metadata_output {
                if scan_file(mp) {
                    contains_ws_root_path = true;
                }
            }
        }
    }

    // 5. Parse dep-info (.d) in staging_dir for extra inputs outside package_dir (P0, P1, P2)
    let extra_inputs_res = extract_unit_extra_inputs(
        unit,
        args.staging_dir,
        ws_root.as_deref(),
        args.out_dir_purity,
        args.out_dir,
    );
    let (extra_inputs, can_publish_cache) = match extra_inputs_res {
        Ok(extra) => (extra, !action_key.contains(":unhashable:")),
        Err(err) => {
            eprintln!(
                "[mitts::warn] Unit '{}' tracked input hashing failed: {}. Discarding cache publication (fail-closed).",
                unit.unit_id(),
                err
            );
            (Vec::new(), false)
        }
    };
    record_extra_inputs_in_ctx(ctx, ws_root.as_deref(), &extra_inputs);

    // 6. Compute result_key and effective action key
    let result_key = if can_publish_cache {
        compute_composite_result_key(action_key, &extra_inputs)
    } else {
        taint_unhashable_key(action_key)
    };

    if let Some(rk_map) = ctx.resolved_keys {
        rk_map
            .lock()
            .unwrap()
            .insert(unit.unit_id(), result_key.clone());
    }

    let effective_action_key = if contains_ws_root_path {
        let root = ws_root.as_ref().unwrap();
        let ws_hash = Sha256::digest(root.to_string_lossy().as_bytes());
        format!("{}:ws_leak:{}", result_key, ws_hash)
    } else {
        result_key.clone()
    };

    // 7. Publish Action Cache entry
    if can_publish_cache && ctx.is_cache_enabled && !cached_outputs.is_empty() {
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let entry = ActionCacheEntry {
            action_key: effective_action_key,
            action_id: unit.unit_id(),
            created_at_secs: now_secs,
            outputs: cached_outputs,
            stdout: String::new(),
            stderr: args.output.stderr.clone(),
            exit_code: 0,
        };
        let has_linkable = |outs: &[CachedOutput]| {
            outs.iter().any(|o| {
                matches!(
                    o.relative_path.extension().and_then(|e| e.to_str()),
                    Some("rlib" | "dylib" | "so")
                )
            })
        };
        let should_put = has_linkable(&entry.outputs)
            || !ctx
                .ac
                .get(&entry.action_key)
                .ok()
                .flatten()
                .is_some_and(|existing| has_linkable(&existing.outputs));
        if should_put {
            let _ = ctx.ac.put(&entry);
        }

        // 8. Record in ActionManifest for mutable units
        if is_mutable {
            let _ = ctx
                .ac
                .record_manifest_entry(action_key, extra_inputs, &result_key);
        }
    }

    Ok(action_outputs)
}

/// Records dep-info extra inputs (`include_str!`, `#[path]`, `env-dep:`) into the ExecutionContext for FastBuildLock.
pub fn record_extra_inputs_in_ctx(
    ctx: &ExecutionContext,
    ws_root: Option<&Path>,
    extra_inputs: &[(String, String)],
) {
    for (path_str, hash) in extra_inputs {
        if let Some(var_name) = path_str.strip_prefix("env:") {
            if var_name == "OUT_DIR"
                || var_name == "CARGO_MANIFEST_DIR"
                || var_name == "CARGO_CRATE_NAME"
                || var_name == "CARGO_BIN_NAME"
                || var_name.starts_with("CARGO_PKG_")
            {
                continue;
            }
            if let Some(env_map) = ctx.tracked_env_vars {
                env_map
                    .lock()
                    .unwrap()
                    .insert(var_name.to_string(), std::env::var(var_name).ok());
            }
        } else if !path_str.starts_with("out:") {
            let file_path = if let Some(rel) = path_str.strip_prefix("ws:") {
                if let Some(root) = ws_root {
                    root.join(rel)
                } else {
                    PathBuf::from(rel)
                }
            } else {
                PathBuf::from(path_str)
            };
            if let Some(files_map) = ctx.tracked_extra_files {
                files_map.lock().unwrap().insert(file_path, hash.clone());
            }
        }
    }
}

/// Appends a unique non-reusable `:unhashable:<nonce>` suffix to taint a key and its dependents.
pub fn taint_unhashable_key(key: &str) -> String {
    static TAINT_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let seq = TAINT_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nonce = super::dag_scheduler::FastPrng::from_env().next_u64() ^ seq;
    format!("{}:unhashable:{:016x}", key, nonce)
}

pub use super::unit_inputs::{
    clean_path, compute_composite_result_key, extract_unit_extra_inputs, resolve_unit_result_key,
};
