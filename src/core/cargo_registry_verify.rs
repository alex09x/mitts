/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Second opinion for a Cargo copy whose mtimes moved past `.cargo-ok`: something rewrote
//! files there (a tool that saves unchanged files, an experiment that restored them), which
//! alone does not make it a different package. The copy is compared with the archive Cargo
//! keeps in `$CARGO_HOME/registry/cache/<index>/<name>-<version>.crate`; a copy whose files
//! all match is the published package. The verdict is remembered under the cache root by a
//! signature of every entry's path, size and mtime, so the archive is read again only after
//! the copy changes.

use super::hash::Sha256;
use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

static VERDICT_DIR: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Where verified signatures are kept (`<cache root>/registry-copies`); none disables memo.
pub fn set_verdict_dir(dir: Option<PathBuf>) {
    *VERDICT_DIR.lock().unwrap_or_else(|e| e.into_inner()) = dir;
}

/// One entry of the copy as the walk saw it.
pub struct Entry {
    pub rel: PathBuf,
    pub path: PathBuf,
    pub mtime_ns: u128,
    pub len: u64,
    pub is_file: bool,
    pub is_symlink: bool,
}

/// True when the copy's files are exactly the archive's (signature memo first).
pub fn matches_published(dir: &Path, entries: &[Entry]) -> bool {
    let signature = signature(dir, entries);
    let memo = memo_path(dir);
    if let Some(memo) = &memo {
        if fs::read_to_string(memo).is_ok_and(|s| s == signature) {
            return true;
        }
    }
    if !matches_archive(dir, entries) {
        return false;
    }
    if let Some(memo) = memo {
        if let Some(parent) = memo.parent() {
            let _ = fs::create_dir_all(parent);
            let tmp = memo.with_extension(format!("tmp{}", std::process::id()));
            if fs::write(&tmp, &signature).is_ok() && fs::rename(&tmp, &memo).is_err() {
                let _ = fs::remove_file(&tmp);
            }
        }
    }
    true
}

fn signature(dir: &Path, entries: &[Entry]) -> String {
    let mut lines: Vec<String> = entries
        .iter()
        .map(|e| {
            format!(
                "{}\0{}\0{}\0{}{}",
                e.rel.display(),
                e.mtime_ns,
                e.len,
                e.is_file as u8,
                e.is_symlink as u8
            )
        })
        .collect();
    lines.sort();
    lines.insert(0, dir.display().to_string());
    Sha256::digest(lines.join("\n").as_bytes())
}

fn memo_path(dir: &Path) -> Option<PathBuf> {
    let root = VERDICT_DIR
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()?;
    Some(root.join(Sha256::digest(dir.display().to_string().as_bytes())))
}

/// `<registry>/src/<index>/<name>-<version>` → `<registry>/cache/<index>/<name>-<version>.crate`.
fn archive_path(dir: &Path) -> Option<PathBuf> {
    let package = dir.file_name()?.to_str()?;
    let index_dir = dir.parent()?;
    let registry = index_dir.parent()?.parent()?;
    Some(
        registry
            .join("cache")
            .join(index_dir.file_name()?)
            .join(format!("{package}.crate")),
    )
}

enum Published {
    File(String),
    Link(PathBuf),
}

fn matches_archive(dir: &Path, entries: &[Entry]) -> bool {
    let Some(published) = archive_path(dir).and_then(|a| read_archive(&a)) else {
        return false;
    };
    let mut seen = 0usize;
    for e in entries {
        if e.is_symlink {
            let Ok(target) = fs::read_link(&e.path) else {
                return false;
            };
            match published.get(&e.rel) {
                Some(Published::Link(t)) if *t == target => seen += 1,
                _ => return false,
            }
        } else if e.is_file {
            let Ok(digest) = Sha256::digest_file(&e.path) else {
                return false;
            };
            match published.get(&e.rel) {
                Some(Published::File(d)) if *d == digest => seen += 1,
                _ => return false,
            }
        }
    }
    seen == published.len()
}

fn read_archive(path: &Path) -> Option<HashMap<PathBuf, Published>> {
    let file = fs::File::open(path).ok()?;
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(file));
    let mut out = HashMap::new();
    for entry in archive.entries().ok()? {
        let mut entry = entry.ok()?;
        let full = entry.path().ok()?.into_owned();
        // `<name>-<version>/...`: the copy is that top directory.
        let rel: PathBuf = full.components().skip(1).collect();
        if rel.as_os_str().is_empty() {
            continue;
        }
        let kind = entry.header().entry_type();
        if kind.is_symlink() {
            out.insert(rel, Published::Link(entry.link_name().ok()??.into_owned()));
        } else if kind.is_file() {
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).ok()?;
            out.insert(rel, Published::File(Sha256::digest(&bytes)));
        }
    }
    Some(out)
}
