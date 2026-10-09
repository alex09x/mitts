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
        let fname = entry.file_name();
        let fname_str = fname.to_string_lossy();

        if fname_str == "target" || fname_str == ".git" || fname_str == ".mitts" {
            continue;
        }

        let Ok(meta) = entry.metadata() else {
            continue;
        };

        if meta.is_dir() {
            collect_project_files(root_dir, &path, manifests, sources, directories)?;
        } else if meta.is_file() {
            if fname_str == "Cargo.toml"
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
            }
        }
    }

    Ok(())
}
