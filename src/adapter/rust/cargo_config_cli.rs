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
//! They form the highest-precedence configuration layer, later values winning: every
//! reader of Cargo configuration takes its files from
//! [`super::cargo_config::discover_cargo_config_files`], which appends these after the
//! discovered `.cargo/config.toml` files. Each inline value is written to its own file, at
//! its position among the values, in a private temporary directory that [`cleanup`]
//! removes when the command ends. Relative paths resolve as [`base_dir`] says.

use anyhow::{anyhow, bail, Context, Result};
use std::path::{Path, PathBuf};
use std::sync::RwLock;

struct Overrides {
    files: Vec<PathBuf>,
    scratch: Option<PathBuf>,
    stamp: String,
}

static OVERRIDES: RwLock<Overrides> = RwLock::new(Overrides {
    files: Vec::new(),
    scratch: None,
    stamp: String::new(),
});

/// Installs the `--config` values of this command (none clears them).
pub fn set(values: &[String]) -> Result<()> {
    cleanup();
    let prepared = prepare(values)?;
    *OVERRIDES.write().unwrap_or_else(|e| e.into_inner()) = prepared;
    super::cargo_config::clear_config_env_cache();
    Ok(())
}

/// Checks the values and writes the inline ones to scratch files, without installing them.
fn prepare(values: &[String]) -> Result<Overrides> {
    let mut overrides = Overrides {
        files: Vec::new(),
        scratch: None,
        stamp: values.join("\u{1f}"),
    };
    // Check everything before writing anything. All inline values together must be one
    // valid table: a key set twice is an error, as in Cargo.
    let mut combined = String::new();
    for value in values {
        if !value.contains('=') {
            if !Path::new(value).is_file() {
                bail!("--config {value}: not a KEY=VALUE pair and not a file");
            }
            continue;
        }
        let parsed: toml::Table = value.parse().map_err(|e| {
            anyhow!("--config {value}: expected a TOML `KEY = VALUE` (e.g. build.jobs=2): {e}")
        })?;
        if parsed.is_empty() {
            bail!("--config {value}: sets nothing");
        }
        combined.push_str(value);
        combined.push('\n');
        combined
            .parse::<toml::Table>()
            .map_err(|e| anyhow!("--config {value} conflicts with an earlier value: {e}"))?;
    }
    for value in values {
        if !value.contains('=') {
            let path = Path::new(value);
            overrides
                .files
                .push(std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf()));
            continue;
        }
        let scratch = match &overrides.scratch {
            Some(dir) => dir.clone(),
            None => {
                let nanos = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0);
                let dir = std::env::temp_dir()
                    .join(format!("mitts-config-{}-{nanos:x}", std::process::id()));
                std::fs::create_dir_all(&dir)
                    .with_context(|| format!("cannot create {}", dir.display()))?;
                overrides.scratch = Some(dir.clone());
                dir
            }
        };
        let file = scratch.join(format!("config-{}.toml", overrides.files.len()));
        std::fs::write(&file, format!("{value}\n"))
            .with_context(|| format!("cannot write {}", file.display()))?;
        overrides.files.push(file);
    }
    Ok(overrides)
}

/// The override files, lowest precedence first.
pub fn files() -> Vec<PathBuf> {
    OVERRIDES
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .files
        .clone()
}

/// Whether `path` holds an inline `--config` value (a scratch file of this run).
pub fn is_inline_file(path: &Path) -> bool {
    OVERRIDES
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .scratch
        .as_deref()
        .is_some_and(|dir| path.starts_with(dir))
}

/// The position of `path` among the `--config` files, if it is one; a stable label for it
/// (the scratch directory's name changes every run).
pub fn position(path: &Path) -> Option<usize> {
    OVERRIDES
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .files
        .iter()
        .position(|file| file == path)
}

/// The directory relative paths in a configuration file are resolved against, as in
/// Cargo: the current directory for an inline `--config` value, the directory holding
/// `.cargo` for `.cargo/config.toml`, and the file's own directory for a `--config FILE`
/// elsewhere.
pub fn base_dir(config_path: &Path) -> PathBuf {
    if is_inline_file(config_path) {
        return std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    }
    let parent = config_path.parent().unwrap_or(Path::new("."));
    if parent.file_name().is_some_and(|name| name == ".cargo") {
        parent.parent().unwrap_or(Path::new(".")).to_path_buf()
    } else {
        parent.to_path_buf()
    }
}

/// The `--config` values, for the options hash (a different override is a different build).
pub fn stamp() -> String {
    OVERRIDES
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .stamp
        .clone()
}

/// Removes the scratch files of inline values and forgets the overrides.
pub fn cleanup() {
    let mut overrides = OVERRIDES.write().unwrap_or_else(|e| e.into_inner());
    if let Some(dir) = overrides.scratch.take() {
        let _ = std::fs::remove_dir_all(dir);
    }
    overrides.files.clear();
    overrides.stamp.clear();
    drop(overrides);
    super::cargo_config::clear_config_env_cache();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `prepare` only: installing would be visible to other tests in this process.
    #[test]
    fn values_keep_their_command_line_order() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("extra.toml");
        std::fs::write(&file, "[env]\nB = \"file\"\n").unwrap();
        let prepared = prepare(&[
            "env.A=\"1\"".to_string(),
            file.display().to_string(),
            "env.B=\"cli\"".to_string(),
        ])
        .unwrap();
        assert_eq!(prepared.files.len(), 3);
        let scratch = prepared.scratch.clone().unwrap();
        assert!(prepared.files[0].starts_with(&scratch));
        assert_eq!(prepared.files[1], std::path::absolute(&file).unwrap());
        // The last value stays last, so it wins over the file before it.
        assert!(prepared.files[2].starts_with(&scratch));
        assert!(std::fs::read_to_string(&prepared.files[2])
            .unwrap()
            .contains("env.B=\"cli\""));
        std::fs::remove_dir_all(scratch).unwrap();

        assert!(prepare(&["not toml =".to_string()]).is_err());
        assert!(prepare(&["missing.toml".to_string()]).is_err());
        let duplicate = prepare(&["a.b=1".to_string(), "a.b=2".to_string()]);
        assert!(duplicate.is_err());
    }

    #[test]
    fn base_dirs_follow_cargo() {
        assert_eq!(
            base_dir(Path::new("/w/.cargo/config.toml")),
            PathBuf::from("/w")
        );
        assert_eq!(base_dir(Path::new("/w/extra.toml")), PathBuf::from("/w"));
    }
}
