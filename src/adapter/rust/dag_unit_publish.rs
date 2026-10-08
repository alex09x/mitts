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

pub(crate) struct PublishArgs<'a> {
    pub unit: &'a PlannedCompilationUnit,
    pub action_key: &'a str,
    pub ctx: &'a ExecutionContext<'a>,
    pub staging_dir: &'a Path,
    pub dest_binary: Option<&'a Path>,
    pub output: &'a RustcOutput,
}

pub(crate) fn publish_unit_artifacts(args: PublishArgs<'_>) -> Result<Vec<PathBuf>> {
    let unit = args.unit;
    let action_key = args.action_key;
    let ctx = args.ctx;
    let clean_crate = unit.target_name.replace('-', "_");
    let mut cached_outputs = Vec::new();
    let mut action_outputs = Vec::new();

    // 1. Move and publish executable binary
    if let Some(dest) = args.dest_binary {
        if let Some(ref bp) = args.output.primary_output {
            if bp.is_file() {
                if let Some(parent) = dest.parent() {
                    let _ = fs::create_dir_all(parent);
                }
                if bp != dest {
                    let _ = crate::core::reflink_or_copy(bp, dest, true);
                }
                if let Ok((bin_hash, bin_size)) = ctx.cas.store_file(dest) {
                    let rel = PathBuf::from(if ctx.opt.release { "release" } else { "debug" })
                        .join(&unit.target_name);
                    cached_outputs.push(CachedOutput {
                        relative_path: rel,
                        cas_hash: bin_hash,
                        file_size: bin_size,
                        executable: true,
                    });
                }
                let alt_path = dest.parent().unwrap_or(dest).join(&clean_crate);
                if alt_path != *dest {
                    let _ = crate::core::reflink_or_copy(dest, &alt_path, true);
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
            let named = ctx.cas.named_artifact_path_with_disambiguator(
                &unit.target_name,
                &hash,
                action_key,
                ext,
            )?;

            cached_outputs.push(CachedOutput {
                relative_path: PathBuf::from(format!("lib{}.{}", clean_crate, ext)),
                cas_hash: hash.clone(),
                file_size: size,
                executable: false,
            });
            ctx.completed_artifacts
                .lock()
                .unwrap()
                .insert(unit.unit_id(), named.clone());
            action_outputs.push(named);

            if !ctx.opt.native {
                let profile_name = if ctx.opt.release { "release" } else { "debug" };
                let deps_dir = ctx.target_dir.join(profile_name).join("deps");
                let _ = fs::create_dir_all(&deps_dir);
                let dep_copy = deps_dir.join(format!("lib{}.{}", clean_crate, ext));
                let _ = crate::core::reflink_or_copy(lib_path, &dep_copy, false);
            }
        }

        // 3. Publish metadata output (rmeta)
        if let Some(ref mp) = args.output.metadata_output {
            let (meta_hash, meta_size) = ctx.cas.store_file(mp)?;
            let _ = ctx.cas.named_artifact_path_with_disambiguator(
                &unit.target_name,
                &meta_hash,
                action_key,
                "rmeta",
            );
            cached_outputs.push(CachedOutput {
                relative_path: PathBuf::from(format!("lib{}.rmeta", clean_crate)),
                cas_hash: meta_hash,
                file_size: meta_size,
                executable: false,
            });
        }
    }

    // 4. Output byte scanning for workspace root leaks (P2)
    let mut contains_ws_root_path = false;
    let ws_root = if unit.is_workspace {
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

    fn clean_path(path: &Path) -> PathBuf {
        use std::path::Component;
        let mut out = PathBuf::new();
        for c in path.components() {
            match c {
                Component::CurDir => {}
                Component::ParentDir => {
                    out.pop();
                }
                _ => out.push(c),
            }
        }
        out
    }

    // 5. Parse dep-info (.d) in staging_dir for extra inputs outside package_dir (P0)
    let mut extra_inputs = Vec::new();
    if unit.is_workspace {
        let root = ws_root.as_ref().unwrap();
        let cleaned_root = clean_path(root);
        let pkg_dir = unit.manifest_path.parent().unwrap_or(Path::new("."));
        let cleaned_pkg = clean_path(pkg_dir);
        if let Ok(entries) = fs::read_dir(args.staging_dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.extension().map(|e| e == "d").unwrap_or(false) {
                    if let Ok(tracked) = super::dep_info::parse_dep_info_with_root(&p, Some(root)) {
                        for file_path in tracked {
                            let cleaned_file = clean_path(&file_path);
                            if cleaned_file.starts_with(&cleaned_pkg) {
                                continue;
                            }
                            let norm_path =
                                if let Ok(rel) = cleaned_file.strip_prefix(&cleaned_root) {
                                    format!("ws:{}", rel.display())
                                } else {
                                    cleaned_file.to_string_lossy().to_string()
                                };
                            if let Ok(hash) = Sha256::digest_file(&file_path) {
                                extra_inputs.push((norm_path, hash));
                            }
                        }
                    }
                }
            }
        }
        extra_inputs.sort_by(|a, b| a.0.cmp(&b.0));
        extra_inputs.dedup_by(|a, b| a.0 == b.0);
    }

    // 6. Compute result_key and effective action key
    let result_key = if extra_inputs.is_empty() {
        action_key.to_string()
    } else {
        let mut hasher_payload = format!("base:{}\nextra:\n", action_key);
        for (path, hash) in &extra_inputs {
            hasher_payload.push_str(&format!("  {}: {}\n", path, hash));
        }
        Sha256::digest(hasher_payload.as_bytes())
    };

    let effective_action_key = if contains_ws_root_path {
        let root = ws_root.as_ref().unwrap();
        let ws_hash = Sha256::digest(root.to_string_lossy().as_bytes());
        format!("{}:ws_leak:{}", result_key, ws_hash)
    } else {
        result_key.clone()
    };

    // 7. Publish Action Cache entry
    if ctx.is_cache_enabled && !cached_outputs.is_empty() {
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
        let _ = ctx.ac.put(&entry);

        // 8. Record in ActionManifest for workspace units
        if unit.is_workspace {
            let _ = ctx
                .ac
                .record_manifest_entry(action_key, extra_inputs, &result_key);
        }
    }

    Ok(action_outputs)
}
