/*
 * Mitts Build — `--config KEY=VALUE` / `--config FILE` overrides
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Like Cargo's `--config`, each value is either a TOML `KEY = VALUE` (dotted keys, e.g.
//! `build.target-dir="out"` or `profile.release.opt-level=1`) or the path of a TOML file.
//! They form the highest-precedence configuration layer: every reader of Cargo
//! configuration takes its files from [`super::cargo_config::discover_cargo_config_files`],
//! which appends these after the discovered `.cargo/config.toml` files, later values
//! winning. Inline values are written to one file in a private temporary directory that
//! [`cleanup`] removes when the command ends. Relative paths in inline values are relative
//! to the current directory, as in Cargo.

use anyhow::{anyhow, bail, Context, Result};
use std::path::{Path, PathBuf};
use std::sync::RwLock;

struct Overrides {
    files: Vec<PathBuf>,
    inline_file: Option<PathBuf>,
    scratch: Option<PathBuf>,
    stamp: String,
}

static OVERRIDES: RwLock<Overrides> = RwLock::new(Overrides {
    files: Vec::new(),
    inline_file: None,
    scratch: None,
    stamp: String::new(),
});

/// Installs the `--config` values of this command (none clears them).
pub fn set(values: &[String]) -> Result<()> {
    cleanup();
    let prepared = prepare(values)?;
    *OVERRIDES.write().unwrap_or_else(|e| e.into_inner()) = prepared;
    Ok(())
}

/// Checks the values and writes the inline ones to a scratch file, without installing them.
fn prepare(values: &[String]) -> Result<Overrides> {
    if values.is_empty() {
        return Ok(Overrides {
            files: Vec::new(),
            inline_file: None,
            scratch: None,
            stamp: String::new(),
        });
    }
    let mut files = Vec::new();
    let mut inline = String::new();
    let mut inline_index = None;
    for value in values {
        let as_path = Path::new(value);
        if !value.contains('=') {
            if !as_path.is_file() {
                bail!("--config {value}: not a KEY=VALUE pair and not a file");
            }
            files.push(std::path::absolute(as_path).unwrap_or_else(|_| as_path.to_path_buf()));
            continue;
        }
        let parsed: toml::Table = value.parse().map_err(|e| {
            anyhow!("--config {value}: expected a TOML `KEY = VALUE` (e.g. build.jobs=2): {e}")
        })?;
        if parsed.is_empty() {
            bail!("--config {value}: sets nothing");
        }
        inline.push_str(value);
        inline.push('\n');
        // Inline values take the place of their first occurrence among the files.
        inline_index.get_or_insert(files.len());
    }
    // Check the inline values together (a key set twice is a TOML error, as in Cargo).
    let mut scratch = None;
    let mut inline_file = None;
    if let Some(index) = inline_index {
        inline
            .parse::<toml::Table>()
            .map_err(|e| anyhow!("--config values conflict: {e}"))?;
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir =
            std::env::temp_dir().join(format!("mitts-config-{}-{nanos:x}", std::process::id()));
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("cannot create {}", dir.display()))?;
        let file = dir.join("config.toml");
        std::fs::write(&file, &inline)
            .with_context(|| format!("cannot write {}", file.display()))?;
        files.insert(index, file.clone());
        scratch = Some(dir);
        inline_file = Some(file);
    }
    Ok(Overrides {
        files,
        inline_file,
        scratch,
        stamp: values.join("\u{1f}"),
    })
}

/// The override files, lowest precedence first.
pub fn files() -> Vec<PathBuf> {
    OVERRIDES
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .files
        .clone()
}

/// The file inline `--config` values were written to, whose relative paths are relative to
/// the current directory rather than to the file.
pub fn is_inline_file(path: &Path) -> bool {
    OVERRIDES
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .inline_file
        .as_deref()
        == Some(path)
}

/// The `--config` values, for the options hash (a different override is a different build).
pub fn stamp() -> String {
    OVERRIDES
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .stamp
        .clone()
}

/// Removes the temporary file of inline values and forgets the overrides.
pub fn cleanup() {
    let mut overrides = OVERRIDES.write().unwrap_or_else(|e| e.into_inner());
    if let Some(dir) = overrides.scratch.take() {
        let _ = std::fs::remove_dir_all(dir);
    }
    overrides.files.clear();
    overrides.inline_file = None;
    overrides.stamp.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `prepare` only: installing would be visible to other tests in this process.
    #[test]
    fn inline_values_and_files_are_layered_in_order() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("extra.toml");
        std::fs::write(&file, "[build]\njobs = 3\n").unwrap();
        let prepared = prepare(&[
            "build.target-dir=\"out\"".to_string(),
            file.display().to_string(),
        ])
        .unwrap();
        assert_eq!(prepared.files.len(), 2);
        assert_eq!(prepared.inline_file.as_ref(), Some(&prepared.files[0]));
        let inline = std::fs::read_to_string(&prepared.files[0]).unwrap();
        assert!(inline.contains("build.target-dir"));
        assert_eq!(prepared.files[1], std::path::absolute(&file).unwrap());
        std::fs::remove_dir_all(prepared.scratch.unwrap()).unwrap();

        assert!(prepare(&["not toml =".to_string()]).is_err());
        assert!(prepare(&["missing.toml".to_string()]).is_err());
        assert!(prepare(&["a.b=1".to_string(), "a.b=2".to_string()]).is_err());
    }
}
