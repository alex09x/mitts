/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::Result;
use std::fs;
use std::path::{Path, PathBuf};

use super::{publish_library_alias, publish_uplifted_library, CachedOutput, PublishArgs};

pub(super) fn publish_link_outputs(
    args: &PublishArgs<'_>,
    artifact_key: &str,
    clean_crate: &str,
) -> Result<(Vec<CachedOutput>, Vec<PathBuf>, Vec<PathBuf>)> {
    let unit = args.unit;
    let ctx = args.ctx;
    let mut source_outputs = collect_link_outputs(args.staging_dir, clean_crate);
    if let Some(primary) = args
        .output
        .primary_output
        .as_ref()
        .filter(|p| is_link_output(p))
    {
        if !source_outputs.contains(primary) {
            source_outputs.push(primary.clone());
        }
    }
    source_outputs.sort_by_key(|path| link_output_priority(path));
    source_outputs.dedup();

    let mut cached_outputs = Vec::new();
    let mut action_outputs = Vec::new();
    let mut preferred_artifact: Option<(u8, PathBuf)> = None;
    for path in &source_outputs {
        let (hash, size) = ctx.cas.store_file(path)?;
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("rlib");
        let named = ctx.cas.named_artifact_path_with_disambiguator(
            &unit.target_name,
            &hash,
            artifact_key,
            ext,
        )?;
        let file_name = path.file_name().unwrap_or_default();
        cached_outputs.push(CachedOutput {
            relative_path: PathBuf::from(file_name),
            cas_hash: hash.clone(),
            file_size: size,
            executable: false,
        });
        action_outputs.push(named.clone());

        let priority = extern_artifact_priority(ext);
        if priority > 0
            && preferred_artifact
                .as_ref()
                .is_none_or(|(current, _)| priority > *current)
        {
            preferred_artifact = Some((priority, named));
        }

        if !ctx.opt.native || unit.is_workspace {
            let profile = if ctx.opt.release { "release" } else { "debug" };
            let is_staticlib = matches!(ext, "a" | "lib");
            let dep_copy = if is_staticlib {
                ctx.target_dir.join(profile).join(file_name)
            } else {
                let deps = ctx.target_dir.join(profile).join("deps");
                fs::create_dir_all(&deps)?;
                deps.join(file_name)
            };
            if let Some(parent) = dep_copy.parent() {
                fs::create_dir_all(parent)?;
            }
            if path != &dep_copy {
                let _ = crate::core::link_intra_workspace(
                    path,
                    &dep_copy,
                    &[args.staging_dir, ctx.target_dir],
                    false,
                );
            }
            if let Some(index) = ctx.target_index {
                let rel = dep_copy
                    .strip_prefix(ctx.target_dir)
                    .map(PathBuf::from)
                    .unwrap_or_else(|_| PathBuf::from(profile).join(file_name));
                index.lock().unwrap().record(
                    rel.to_string_lossy().to_string(),
                    hash.clone(),
                    &dep_copy,
                );
            }
            if is_staticlib {
                action_outputs.push(dep_copy);
            } else {
                publish_library_alias(ctx, &dep_copy, clean_crate, ext, &hash);
                if unit.is_workspace {
                    publish_uplifted_library(ctx, &dep_copy, clean_crate, ext, &hash);
                }
            }
        }
    }

    if let Some((_, artifact)) = preferred_artifact {
        ctx.completed_artifacts
            .lock()
            .unwrap()
            .insert(unit.unit_id(), artifact);
    }
    Ok((cached_outputs, action_outputs, source_outputs))
}

fn collect_link_outputs(staging_dir: &Path, clean_crate: &str) -> Vec<PathBuf> {
    let prefix = format!("lib{clean_crate}");
    fs::read_dir(staging_dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            is_link_output(path)
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(&prefix))
        })
        .collect()
}

fn is_link_output(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| matches!(ext, "rlib" | "dylib" | "so" | "a" | "lib"))
}

fn link_output_priority(path: &Path) -> u8 {
    match path.extension().and_then(|e| e.to_str()) {
        Some("rlib") => 0,
        Some("dylib" | "so") => 1,
        Some("a" | "lib") => 2,
        _ => 3,
    }
}

fn extern_artifact_priority(ext: &str) -> u8 {
    match ext {
        "rlib" => 3,
        "dylib" | "so" => 2,
        "a" | "lib" => 1,
        _ => 0,
    }
}
