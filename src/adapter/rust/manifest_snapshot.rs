/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Invocation-scoped manifest bytes shared by planning, keying and DAG workers.

use super::manifest::CargoToml;
use crate::core::hash::Sha256;
use anyhow::{bail, Context, Result};
use std::cell::RefCell;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex, OnceLock};

type Cached<T> = OnceLock<std::result::Result<T, String>>;

struct Entry {
    bytes: Arc<str>,
    parsed: Cached<CargoToml>,
    digests: Cached<(String, String)>,
    raw_digest: OnceLock<String>,
}

#[derive(Default)]
struct Contents {
    entries: Mutex<HashMap<PathBuf, Arc<Entry>>>,
    probes: Mutex<HashMap<PathBuf, bool>>,
    errors: Mutex<Vec<String>>,
}

/// Immutable first-observed manifest bytes for one plan, never a persistent cache.
#[derive(Clone, Default)]
pub struct ManifestSnapshot(Arc<Contents>);

impl std::fmt::Debug for ManifestSnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ManifestSnapshot")
            .field("captured_files", &self.captured_file_count())
            .finish_non_exhaustive()
    }
}

thread_local! {
    static CURRENT: RefCell<Option<ManifestSnapshot>> = const { RefCell::new(None) };
}

/// Restores the calling thread's previous snapshot, including during unwinding.
pub struct ManifestScope {
    snapshot: ManifestSnapshot,
    previous: Option<ManifestSnapshot>,
    _thread_bound: PhantomData<Rc<()>>,
}

impl ManifestSnapshot {
    pub fn enter(&self) -> ManifestScope {
        let previous = CURRENT.with(|slot| slot.replace(Some(self.clone())));
        ManifestScope {
            snapshot: self.clone(),
            previous,
            _thread_bound: PhantomData,
        }
    }

    /// Number of distinct manifest buffers captured, excluding freshness reads.
    pub fn captured_file_count(&self) -> usize {
        self.0.entries.lock().unwrap().len()
    }

    fn path(path: &Path) -> Result<PathBuf> {
        Ok(if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()?.join(path)
        })
    }

    fn entry(&self, path: &Path) -> Result<Arc<Entry>> {
        let path = Self::path(path)?;
        let mut entries = self.0.entries.lock().unwrap();
        if let Some(entry) = entries.get(&path) {
            return Ok(Arc::clone(entry));
        }
        // Coalesce concurrent first readers; parsing and inheritance happen after
        // releasing this lock, so an ancestor can safely use this same snapshot.
        let bytes = match std::fs::read_to_string(&path) {
            Ok(bytes) => bytes,
            Err(error) => {
                let message = format!("Failed to read manifest {}: {error}", path.display());
                self.0.errors.lock().unwrap().push(message.clone());
                bail!(message);
            }
        };
        let entry = Arc::new(Entry {
            bytes: Arc::from(bytes),
            parsed: OnceLock::new(),
            digests: OnceLock::new(),
            raw_digest: OnceLock::new(),
        });
        entries.insert(path, Arc::clone(&entry));
        Ok(entry)
    }

    fn is_file(&self, path: &Path) -> bool {
        let path = match Self::path(path) {
            Ok(path) => path,
            Err(error) => {
                self.0.errors.lock().unwrap().push(error.to_string());
                return false;
            }
        };
        let mut probes = self.0.probes.lock().unwrap();
        if let Some(found) = probes.get(&path) {
            return *found;
        }
        let found = match std::fs::metadata(&path) {
            Ok(metadata) => metadata.is_file(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => {
                self.0.errors.lock().unwrap().push(format!(
                    "Cannot account for manifest {}: {error}",
                    path.display()
                ));
                false
            }
        };
        probes.insert(path, found);
        found
    }

    /// Reject a plan whose captured inputs or ancestor-manifest presence changed.
    /// This deliberately reads disk rather than trusting timestamps or this table.
    pub fn verify_fresh(&self) -> Result<()> {
        let _timer = crate::core::metrics::InputValidationTimer::start();
        if let Some(error) = self.0.errors.lock().unwrap().first() {
            bail!("Manifest snapshot has an unknown input: {error}");
        }
        let entries = self.0.entries.lock().unwrap().clone();
        let probes = self.0.probes.lock().unwrap().clone();
        for (path, entry) in &entries {
            let current = std::fs::read(path).with_context(|| {
                format!("Manifest changed while using plan: {}", path.display())
            })?;
            if current != entry.bytes.as_bytes() {
                bail!("Manifest changed while using plan: {}", path.display());
            }
        }
        for (path, was_file) in probes {
            if entries.contains_key(&path) {
                continue;
            }
            let is_file = match std::fs::metadata(&path) {
                Ok(metadata) => metadata.is_file(),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
                Err(error) => return Err(error).context("Cannot verify manifest presence"),
            };
            if is_file != was_file {
                bail!(
                    "Ancestor manifest changed while using plan: {}",
                    path.display()
                );
            }
        }
        Ok(())
    }
}

impl ManifestScope {
    pub(crate) fn snapshot(&self) -> ManifestSnapshot {
        self.snapshot.clone()
    }
}

impl Drop for ManifestScope {
    fn drop(&mut self) {
        CURRENT.with(|slot| slot.replace(self.previous.take()));
    }
}

pub(crate) fn current() -> Option<ManifestSnapshot> {
    CURRENT.with(|slot| slot.borrow().clone())
}

pub(crate) fn scope() -> ManifestScope {
    current().unwrap_or_default().enter()
}

pub(crate) fn read_text(path: &Path) -> Result<Arc<str>> {
    match current() {
        Some(snapshot) => Ok(Arc::clone(&snapshot.entry(path)?.bytes)),
        None => Ok(Arc::from(
            std::fs::read_to_string(path).with_context(|| format!("Failed to read {:?}", path))?,
        )),
    }
}

pub(crate) fn is_manifest_file(path: &Path) -> bool {
    current().map_or_else(|| path.is_file(), |snapshot| snapshot.is_file(path))
}

pub(crate) fn parse(path: &Path) -> Result<CargoToml> {
    let parse_bytes = |bytes: &str| -> Result<CargoToml> {
        let mut manifest = CargoToml::parse(bytes)?;
        manifest.resolve_inherited_workspace_fields(path);
        Ok(manifest)
    };
    if let Some(snapshot) = current() {
        let entry = snapshot.entry(path)?;
        return entry
            .parsed
            .get_or_init(|| parse_bytes(&entry.bytes).map_err(|error| error.to_string()))
            .clone()
            .map_err(anyhow::Error::msg);
    }
    parse_bytes(&read_text(path)?)
}

pub(crate) fn raw_digest(path: &Path) -> Result<String> {
    if let Some(snapshot) = current() {
        let entry = snapshot.entry(path)?;
        return Ok(entry
            .raw_digest
            .get_or_init(|| Sha256::digest(entry.bytes.as_bytes()))
            .clone());
    }
    Sha256::digest_file(path)
}

pub(crate) fn digests(path: &Path) -> Result<(String, String)> {
    if let Some(snapshot) = current() {
        let entry = snapshot.entry(path)?;
        return entry
            .digests
            .get_or_init(|| {
                super::manifest_digest::digests_from_content(&entry.bytes)
                    .map_err(|error| error.to_string())
            })
            .clone()
            .map_err(anyhow::Error::msg);
    }
    super::manifest_digest::digests_from_content(&read_text(path)?)
}
