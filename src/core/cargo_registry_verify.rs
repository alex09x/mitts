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
    /// `cargo_registry_copy::stamp`: the status-change time on Unix.
    pub stamp_ns: u128,
    pub len: u64,
    pub is_file: bool,
    pub is_symlink: bool,
    pub executable: bool,
}

/// True when the copy's files are exactly the archive's (signature memo first).
pub fn matches_published(dir: &Path, entries: &[Entry]) -> bool {
    // Both verdicts are remembered: an edited copy (a stubbed skia-bindings) would
    // otherwise re-read a large archive on every plan. The signature covers the archive
    // too, so a verdict made without one is not reused once Cargo downloads it.
    let signature = signature(dir, entries);
    let memo = memo_path(dir);
    if let Some(memo) = &memo {
        if let Ok(saved) = fs::read_to_string(memo) {
            match saved.split_once('\n') {
                Some(("published", s)) if s == signature => return true,
                Some(("edited", s)) if s == signature => return false,
                _ => {}
            }
        }
    }
    let verdict = compare_with_archive(dir, entries);
    // A read that failed proves nothing: not remembered, so the next plan looks again.
    if let (Some(memo), Some(published)) = (memo, verdict) {
        let word = if published { "published" } else { "edited" };
        write_atomically(&memo, &format!("{word}\n{signature}"));
    }
    verdict.unwrap_or(false)
}

fn write_atomically(path: &Path, contents: &str) {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let Some(parent) = path.parent() else { return };
    let _ = fs::create_dir_all(parent);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = path.with_extension(format!("tmp{}-{seq}", std::process::id()));
    if fs::write(&tmp, contents).is_ok() && fs::rename(&tmp, path).is_err() {
        let _ = fs::remove_file(&tmp);
    }
}

fn signature(dir: &Path, entries: &[Entry]) -> String {
    let mut lines: Vec<String> = entries
        .iter()
        .map(|e| {
            format!(
                "{}\0{}\0{}\0{}{}{}",
                e.rel.display(),
                e.stamp_ns,
                e.len,
                e.is_file as u8,
                e.is_symlink as u8,
                e.executable as u8
            )
        })
        .collect();
    lines.sort();
    let archive = archive_path(dir)
        .and_then(|a| fs::metadata(a).ok())
        .map(|m| {
            let stamp = super::cargo_registry_copy::stamp(&m);
            format!("archive\0{}\0{stamp}", m.len())
        })
        .unwrap_or_else(|| "no archive".to_string());
    lines.insert(0, archive);
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
    /// Content digest and the owner's execute bit.
    File(String, bool),
    Link(PathBuf),
}

/// `Some(true)` the archive's files exactly, `Some(false)` a difference or no archive to
/// compare with, `None` a read failed (no verdict).
fn compare_with_archive(dir: &Path, entries: &[Entry]) -> Option<bool> {
    let archive = archive_path(dir)?;
    if !archive.is_file() {
        return Some(false);
    }
    let published = read_archive(&archive)?;
    let mut seen = 0usize;
    for e in entries {
        if e.is_symlink {
            let target = fs::read_link(&e.path).ok()?;
            match published.get(&e.rel) {
                Some(Published::Link(t)) if *t == target => seen += 1,
                _ => return Some(false),
            }
        } else if e.is_file {
            let digest = Sha256::digest_file(&e.path).ok()?;
            // Cargo unpacks the archive's modes (umask aside); only Unix has the bit.
            let same_mode = |exec: bool| !cfg!(unix) || exec == e.executable;
            match published.get(&e.rel) {
                Some(Published::File(d, exec)) if *d == digest && same_mode(*exec) => seen += 1,
                _ => return Some(false),
            }
        }
    }
    Some(seen == published.len())
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
            let exec = entry.header().mode().is_ok_and(|m| m & 0o100 != 0);
            // Streamed: a large member is never held in memory whole.
            let digest = Sha256::digest_reader(&mut entry).ok()?;
            out.insert(rel, Published::File(digest, exec));
        }
    }
    Some(out)
}
