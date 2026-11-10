/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

mod aliases;
mod inputs;
mod keys;
mod multi_crate_outputs;

pub(crate) use aliases::{publish_library_alias, publish_uplifted_library};
pub use inputs::record_extra_inputs_in_ctx;
pub use keys::strip_unhashable_taint;
pub(crate) use keys::{artifact_disambiguator, bind_interface_digest};

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
    if unit.kind == "example" {
        target_dir
            .join(profile)
            .join("examples")
            .join(&unit.target_name)
    } else if unit.kind == "test" || unit.kind == "custom-test" {
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
    let mut link_output_sources = Vec::new();

    let is_mutable = super::unit_inputs::is_mutable_unit(unit);
    let ws_root = if is_mutable {
        Some(super::compiler::find_workspace_root(&unit.manifest_path))
    } else {
        None
    };

    // 1. Parse dep-info (.d) in staging_dir for extra inputs outside package_dir (P0, P1, P2)
    let extra_inputs_res = extract_unit_extra_inputs(
        unit,
        args.staging_dir,
        ws_root.as_deref(),
        args.out_dir_purity,
        args.out_dir,
    );
    let (extra_inputs, can_publish_cache) = match extra_inputs_res {
        Ok(extra) => (
            extra,
            ctx.is_cache_enabled && !action_key.contains(":unhashable:"),
        ),
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

    // 2. Compute result_key; shared artifact names follow it, not the coarse action key
    let deterministic_rk =
        compute_composite_result_key(&strip_unhashable_taint(action_key), &extra_inputs);
    let result_key = if can_publish_cache {
        deterministic_rk
    } else {
        taint_unhashable_key(&deterministic_rk)
    };

    let artifact_key = artifact_disambiguator(&result_key, Some(args.staging_dir));

    // 3. Store executable binary in CAS from staging first (N3), then materialize in target/
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

                action_outputs.push(dest.to_path_buf());
            }
        }
    } else {
        let (outputs, paths, sources) =
            multi_crate_outputs::publish_link_outputs(&args, &artifact_key, &clean_crate)?;
        cached_outputs.extend(outputs);
        action_outputs.extend(paths);
        link_output_sources = sources;

        // 5. Publish metadata output, including check-only output also used as primary_output.
        if let Some(mp) = args.output.metadata_output.as_ref() {
            let (meta_hash, meta_size) = ctx.cas.store_file(mp)?;
            let file_name = mp.file_name().unwrap_or_default();
            cached_outputs.push(CachedOutput {
                relative_path: PathBuf::from(file_name),
                cas_hash: meta_hash.clone(),
                file_size: meta_size,
                executable: false,
            });
            if super::build_deps::is_importable_crate_unit(unit) {
                let metadata_disambiguator = if link_output_sources.is_empty() {
                    format!("{artifact_key}-m")
                } else {
                    artifact_key.clone()
                };
                let named = ctx.cas.named_artifact_path_with_disambiguator(
                    &unit.target_name,
                    &meta_hash,
                    &metadata_disambiguator,
                    "rmeta",
                )?;
                let extern_meta = if let Some(ref bdeps) = super::build_deps::active_dir() {
                    super::build_deps::materialize_in_dir(
                        ctx.cas,
                        bdeps,
                        Path::new(file_name),
                        &meta_hash,
                    )?
                } else {
                    named
                };
                let mut completed = ctx.completed_artifacts.lock().unwrap();
                if link_output_sources.is_empty() {
                    completed.insert(unit.unit_id(), extern_meta);
                } else {
                    completed.entry(unit.unit_id()).or_insert(extern_meta);
                }
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
                        let rel_key =
                            format!("{}/deps/{}", profile_name, file_name.to_string_lossy());
                        ti.lock()
                            .unwrap()
                            .record(rel_key, meta_hash.clone(), &dep_copy);
                    }
                    publish_library_alias(ctx, &dep_copy, &clean_crate, "rmeta", &meta_hash);
                }
            }
        }
    }

    // Dependents key on the bytes they compile against, not on the result key alone.
    if let Some(rk_map) = ctx.resolved_keys {
        let bound = bind_interface_digest(
            &result_key,
            cached_outputs
                .iter()
                .map(|o| (o.relative_path.as_path(), o.cas_hash.as_str())),
        );
        rk_map.lock().unwrap().insert(unit.unit_id(), bound);
    }

    // Declared wrapper outputs travel with the entry (bound above to rustc's outputs only,
    // as on restore). If they cannot be stored, the entry is not published (fail closed).
    let mut can_publish_cache = can_publish_cache;
    match super::wrapper_outputs::collect(ctx.cas, args.staging_dir) {
        Ok(extra) => cached_outputs.extend(extra),
        Err(err) => {
            eprintln!(
                "[mitts::warn] Unit '{}': storing the rustc wrapper's declared outputs failed: {err:#}. Not caching it.",
                unit.unit_id()
            );
            can_publish_cache = false;
        }
    }

    // 6. Output byte scanning for workspace root leaks (P2): an artifact that embeds the
    // checkout path is published under a per-checkout key, and a marker lets later misses
    // in other checkouts take a per-checkout lock instead of queueing on the shared one.
    let mut contains_ws_root_path = false;

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
            if link_output_sources
                .iter()
                .any(|path| scan_file(path.as_path()))
            {
                contains_ws_root_path = true;
            }
        }
    }

    let effective_action_key = if contains_ws_root_path {
        let root = ws_root.as_ref().unwrap();
        // Keyed by the coarse action key: that is what a miss in another checkout
        // knows before rustc has run.
        mark_ws_leak(ctx.ac.root(), action_key);
        format!("{}:ws_leak:{}", result_key, workspace_hash(root))
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

/// Hash of a checkout root as used in `:ws_leak:` action keys.
pub(crate) fn workspace_hash(ws_root: &Path) -> String {
    Sha256::digest(ws_root.to_string_lossy().as_bytes())
}

fn ws_leak_marker(ac_root: &Path, result_key: &str) -> PathBuf {
    ac_root
        .join("ws_leak")
        .join(Sha256::digest(result_key.as_bytes()))
}

/// Records that some checkout produced a path-embedding artifact for the coarse `action_key`.
pub(crate) fn mark_ws_leak(ac_root: &Path, action_key: &str) {
    let marker = ws_leak_marker(ac_root, action_key);
    if let Some(dir) = marker.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let _ = fs::write(marker, b"");
}

/// The key a miss should take its single-flight lock on: the per-checkout `:ws_leak:` key
/// once any checkout has shown the artifact embeds its path (those builds cannot share a
/// result, so they must not wait for each other), otherwise `result_key` itself.
pub(crate) fn single_flight_action_key(ac_root: &Path, action_key: &str, ws_root: &Path) -> String {
    if ws_leak_marker(ac_root, action_key).is_file() {
        format!("{}:ws_leak:{}", action_key, workspace_hash(ws_root))
    } else {
        action_key.to_string()
    }
}
