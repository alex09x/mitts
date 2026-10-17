/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::build_lock::FileSnapshot;
use super::hash::Sha256;
use anyhow::Result;
use std::fs;
use std::path::Path;
use std::time::SystemTime;

pub(crate) fn mtime_parts(meta: &fs::Metadata) -> (u64, u32) {
    if let Ok(m) = meta.modified() {
        if let Ok(dur) = m.duration_since(SystemTime::UNIX_EPOCH) {
            return (dur.as_secs(), dur.subsec_nanos());
        }
    }
    (0, 0)
}

pub(crate) fn collect_project_files(
    root_dir: &Path,
    current_dir: &Path,
    target_dir: Option<&Path>,
    manifests: &mut Vec<FileSnapshot>,
    sources: &mut Vec<FileSnapshot>,
    directories: &mut Vec<FileSnapshot>,
) -> Result<()> {
    if let Ok(meta) = fs::metadata(current_dir) {
        let (s, n) = mtime_parts(&meta);
        let rel = current_dir
            .strip_prefix(root_dir)
            .unwrap_or(current_dir)
            .to_path_buf();
        directories.push(FileSnapshot {
            relative_path: rel,
            mtime_secs: s,
            mtime_nanos: n,
            size: meta.len(),
            sha256: None,
        });
    }

    let Ok(entries) = fs::read_dir(current_dir) else {
        return Ok(());
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if target_dir.is_some_and(|td| path == td) {
            continue;
        }
        let fname = entry.file_name();
        let fname_str = fname.to_string_lossy();

        if matches!(
            fname_str.as_ref(),
            "target" | ".git" | ".mitts" | ".cargo" | ".cargo-home"
        ) {
            continue;
        }

        let Ok(meta) = entry.metadata() else {
            continue;
        };

        if meta.is_dir() {
            collect_project_files(root_dir, &path, target_dir, manifests, sources, directories)?;
        } else if meta.is_file() {
            if fname_str.ends_with(".tmp")
                || fname_str.ends_with(".swp")
                || fname_str.ends_with(".profraw")
                || fname_str.ends_with(".profdata")
            {
                continue;
            }
            if fname_str == "Cargo.toml"
                || fname_str == "Cargo.lock"
                || fname_str == "clippy.toml"
                || fname_str == ".clippy.toml"
            {
                let (s, n) = mtime_parts(&meta);
                let hash = fs::read(&path).ok().map(|b| Sha256::digest(&b));
                let rel = path.strip_prefix(root_dir).unwrap_or(&path).to_path_buf();
                manifests.push(FileSnapshot {
                    relative_path: rel,
                    mtime_secs: s,
                    mtime_nanos: n,
                    size: meta.len(),
                    sha256: hash,
                });
            } else if fname_str.ends_with(".rs") {
                let (s, n) = mtime_parts(&meta);
                let rel = path.strip_prefix(root_dir).unwrap_or(&path).to_path_buf();
                sources.push(FileSnapshot {
                    relative_path: rel,
                    mtime_secs: s,
                    mtime_nanos: n,
                    size: meta.len(),
                    sha256: None,
                });
            } else {
                let (s, n) = mtime_parts(&meta);
                let hash = if meta.len() <= 1024 * 1024 {
                    fs::read(&path).ok().map(|b| Sha256::digest(&b))
                } else {
                    None
                };
                let rel = path.strip_prefix(root_dir).unwrap_or(&path).to_path_buf();
                sources.push(FileSnapshot {
                    relative_path: rel,
                    mtime_secs: s,
                    mtime_nanos: n,
                    size: meta.len(),
                    sha256: hash,
                });
            }
        }
    }

    Ok(())
}

pub(crate) fn collect_config_snapshots(
    project_dir: &Path,
) -> (Vec<FileSnapshot>, Vec<FileSnapshot>) {
    let config_start_dir =
        crate::adapter::rust::cargo_config::resolve_config_start_dir(project_dir);
    let discovered_configs =
        crate::adapter::rust::cargo_config::discover_cargo_config_files(&config_start_dir);
    let mut cargo_configs = Vec::new();
    for cfg_p in discovered_configs {
        if let Ok(meta) = fs::metadata(&cfg_p) {
            let (s, n) = mtime_parts(&meta);
            let hash = fs::read(&cfg_p).ok().map(|b| Sha256::digest(&b));
            cargo_configs.push(FileSnapshot {
                relative_path: cfg_p,
                mtime_secs: s,
                mtime_nanos: n,
                size: meta.len(),
                sha256: hash,
            });
        }
    }

    let mut clippy_configs = Vec::new();
    if let Some(cp) = crate::adapter::rust::inputs::discover_clippy_config(project_dir) {
        if let Ok(meta) = fs::metadata(&cp) {
            let (s, n) = mtime_parts(&meta);
            let hash = fs::read(&cp).ok().map(|b| Sha256::digest(&b));
            clippy_configs.push(FileSnapshot {
                relative_path: cp,
                mtime_secs: s,
                mtime_nanos: n,
                size: meta.len(),
                sha256: hash,
            });
        }
    }
    (cargo_configs, clippy_configs)
}

pub(crate) fn verify_config_snapshots(
    project_dir: &Path,
    cargo_configs: &[FileSnapshot],
    clippy_configs: &[FileSnapshot],
) -> std::result::Result<(), String> {
    let config_start_dir =
        crate::adapter::rust::cargo_config::resolve_config_start_dir(project_dir);
    let current_configs =
        crate::adapter::rust::cargo_config::discover_cargo_config_files(&config_start_dir);
    if current_configs.len() != cargo_configs.len() {
        return Err("Cargo configuration files changed (added/removed)".to_string());
    }
    for cfg_snap in cargo_configs {
        let full_p = &cfg_snap.relative_path;
        let Ok(meta) = fs::metadata(full_p) else {
            return Err(format!("Cargo config missing: {}", full_p.display()));
        };
        let (m_secs, m_nanos) = mtime_parts(&meta);
        if (m_secs, m_nanos) != (cfg_snap.mtime_secs, cfg_snap.mtime_nanos)
            || meta.len() != cfg_snap.size
        {
            let content_hash = fs::read(full_p).ok().map(|b| Sha256::digest(&b));
            if content_hash != cfg_snap.sha256 {
                return Err(format!("Cargo config modified: {}", full_p.display()));
            }
        }
    }
    for cfg_p in &current_configs {
        if !cargo_configs.iter().any(|c| &c.relative_path == cfg_p) {
            return Err(format!("New Cargo config added: {}", cfg_p.display()));
        }
    }

    let current_clippy = crate::adapter::rust::inputs::discover_clippy_config(project_dir);
    let expected_clippy_count = if current_clippy.is_some() { 1 } else { 0 };
    if expected_clippy_count != clippy_configs.len() {
        return Err("Clippy configuration file changed (added/removed)".to_string());
    }
    for cfg_snap in clippy_configs {
        let full_p = &cfg_snap.relative_path;
        let Ok(meta) = fs::metadata(full_p) else {
            return Err(format!("Clippy config missing: {}", full_p.display()));
        };
        let (m_secs, m_nanos) = mtime_parts(&meta);
        if (m_secs, m_nanos) != (cfg_snap.mtime_secs, cfg_snap.mtime_nanos)
            || meta.len() != cfg_snap.size
        {
            let content_hash = fs::read(full_p).ok().map(|b| Sha256::digest(&b));
            if content_hash != cfg_snap.sha256 {
                return Err(format!("Clippy config modified: {}", full_p.display()));
            }
        }
    }
    if let Some(ref cp) = current_clippy {
        if !clippy_configs.iter().any(|c| &c.relative_path == cp) {
            return Err(format!("New Clippy config added: {}", cp.display()));
        }
    }
    Ok(())
}
