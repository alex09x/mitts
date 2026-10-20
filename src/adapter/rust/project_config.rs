/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Project-level Mitts settings, read from the workspace root `Cargo.toml`.
//!
//! Cargo ignores `metadata` tables, so `[workspace.metadata.mitts]` (or
//! `[package.metadata.mitts]` in a single-package project) is where a project
//! tells Mitts what Cargo cannot know about it:
//!
//! ```toml
//! [workspace.metadata.mitts]
//! impure-proc-macros = ["sqlx", "askama", "rust-embed"]
//! ```
//!
//! `impure-proc-macros` names crates whose macros read files or the package
//! path while expanding. A unit that depends on one of them directly is keyed on
//! the whole package tree and on the absolute manifest dir, so an edited template
//! rebuilds it and another checkout never reuses it. Every other macro is treated
//! as pure: it sees only the tokens it is given, and its dependents get
//! path-independent keys that are shared across worktrees.

use anyhow::{bail, Context, Result};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

use super::types::PlannedCompilationUnit;

const TABLE: &str = "mitts";
const IMPURE_PROC_MACROS: &str = "impure-proc-macros";
const SUPPORTED_KEYS: &[&str] = &[IMPURE_PROC_MACROS];

/// Settings a project declares for Mitts in its root manifest.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ProjectConfig {
    /// Crates whose macros read package files or paths during expansion,
    /// normalized with `-` replaced by `_`.
    pub impure_proc_macros: BTreeSet<String>,
    /// Problems that do not stop a build, such as an unknown key; `load` prints
    /// them to stderr once per read of the manifest.
    pub warnings: Vec<String>,
}

impl ProjectConfig {
    /// Parses the `metadata.mitts` tables of a root manifest. A manifest without
    /// them, or one that does not parse as TOML, yields the empty config.
    pub fn from_manifest_str(content: &str, manifest_path: &Path) -> Result<Self> {
        let Ok(doc) = toml::from_str::<toml::Table>(content) else {
            return Ok(Self::default());
        };
        let mut cfg = Self::default();
        for parent in ["workspace", "package"] {
            let Some(table) = doc
                .get(parent)
                .and_then(|t| t.get("metadata"))
                .and_then(|t| t.get(TABLE))
            else {
                continue;
            };
            let label = format!("[{parent}.metadata.{TABLE}]");
            let Some(table) = table.as_table() else {
                bail!("{}: {label} must be a table", manifest_path.display());
            };
            for (key, value) in table {
                if key.as_str() == IMPURE_PROC_MACROS {
                    cfg.impure_proc_macros
                        .extend(string_list(value, &label, key, manifest_path)?);
                } else {
                    cfg.warnings.push(format!(
                        "{}: {label} has an unknown key {key:?} (supported: {})",
                        manifest_path.display(),
                        SUPPORTED_KEYS.join(", ")
                    ));
                }
            }
        }
        Ok(cfg)
    }

    /// True when the unit directly depends on a crate declared impure, whether the
    /// facade (`sqlx`) or the macro crate itself (`sqlx-macros`) is listed.
    pub fn unit_has_impure_proc_macro(&self, unit: &PlannedCompilationUnit) -> bool {
        if self.impure_proc_macros.is_empty() {
            return false;
        }
        unit.dependencies.iter().any(|dep| {
            let name = dep.split('@').next().unwrap_or("");
            self.impure_proc_macros.contains(&normalize(name))
        })
    }
}

fn string_list(
    value: &toml::Value,
    label: &str,
    key: &str,
    manifest_path: &Path,
) -> Result<Vec<String>> {
    let Some(items) = value.as_array() else {
        bail!(
            "{}: {label} {key} must be an array of crate names",
            manifest_path.display()
        );
    };
    items
        .iter()
        .map(|item| match item.as_str() {
            Some(s) if !s.trim().is_empty() => Ok(normalize(s.trim())),
            _ => bail!(
                "{}: {label} {key} must contain only crate names, got {item}",
                manifest_path.display()
            ),
        })
        .collect()
}

fn normalize(name: &str) -> String {
    name.replace('-', "_")
}

struct CachedConfig {
    stamp: Option<(SystemTime, u64)>,
    config: Arc<ProjectConfig>,
}

fn cache() -> &'static Mutex<HashMap<PathBuf, CachedConfig>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, CachedConfig>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn manifest_stamp(path: &Path) -> Option<(SystemTime, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

/// Loads the config of the workspace rooted at `root`, re-reading the manifest
/// only when its size or modification time changed. The cache lock is held
/// across the read so parallel workers that miss together parse, and warn, once.
pub fn load(root: &Path) -> Result<Arc<ProjectConfig>> {
    let manifest = root.join("Cargo.toml");
    let stamp = manifest_stamp(&manifest);
    let mut cache = cache().lock().unwrap();
    if let Some(entry) = cache.get(&manifest) {
        if entry.stamp == stamp {
            return Ok(Arc::clone(&entry.config));
        }
    }
    let config = if manifest.is_file() {
        let content = std::fs::read_to_string(&manifest)
            .with_context(|| format!("Failed to read {}", manifest.display()))?;
        let cfg = ProjectConfig::from_manifest_str(&content, &manifest)?;
        for warning in &cfg.warnings {
            eprintln!("[mitts] warning: {warning}");
        }
        Arc::new(cfg)
    } else {
        Arc::new(ProjectConfig::default())
    };
    cache.insert(
        manifest,
        CachedConfig {
            stamp,
            config: Arc::clone(&config),
        },
    );
    Ok(config)
}

/// Loads the config governing `unit`: `ws_root` when the caller knows it, else the
/// unit's recorded workspace root, else the root found above its manifest.
pub fn for_unit(
    unit: &PlannedCompilationUnit,
    ws_root: Option<&Path>,
) -> Result<Arc<ProjectConfig>> {
    let root = match (ws_root, unit.workspace_root.as_deref()) {
        (Some(r), _) | (None, Some(r)) => r.to_path_buf(),
        (None, None) => super::compiler::find_workspace_root(&unit.manifest_path),
    };
    load(&root)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit_with_deps(deps: &[&str]) -> PlannedCompilationUnit {
        PlannedCompilationUnit {
            dependencies: deps.iter().map(|d| d.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn manifest_without_mitts_tables_is_empty() {
        let cfg =
            ProjectConfig::from_manifest_str("[workspace]\nmembers = [\"a\"]\n", Path::new("x"))
                .unwrap();
        assert_eq!(cfg, ProjectConfig::default());
        let cfg = ProjectConfig::from_manifest_str("not = toml = at all", Path::new("x")).unwrap();
        assert_eq!(cfg, ProjectConfig::default());
    }

    #[test]
    fn workspace_and_package_tables_merge_and_normalize_names() {
        let content = r#"
[workspace]
members = ["app"]
[workspace.metadata.mitts]
impure-proc-macros = ["sqlx", "rust-embed"]
[package]
name = "root"
version = "0.1.0"
[package.metadata.mitts]
impure-proc-macros = [" askama_derive "]
"#;
        let cfg = ProjectConfig::from_manifest_str(content, Path::new("x")).unwrap();
        let names: Vec<&str> = cfg.impure_proc_macros.iter().map(String::as_str).collect();
        assert_eq!(names, ["askama_derive", "rust_embed", "sqlx"]);
    }

    #[test]
    fn unknown_keys_in_the_declaration_warn_but_do_not_fail() {
        let content = r#"
[workspace.metadata.mitts]
impure-proc-macro = ["sqlx"]
foo = 1
"#;
        let cfg = ProjectConfig::from_manifest_str(content, Path::new("ws/Cargo.toml")).unwrap();
        assert!(cfg.impure_proc_macros.is_empty(), "{cfg:?}");
        assert_eq!(cfg.warnings.len(), 2, "{cfg:?}");
        assert_eq!(
            cfg.warnings[0],
            "ws/Cargo.toml: [workspace.metadata.mitts] has an unknown key \"foo\" (supported: impure-proc-macros)"
        );
        assert!(
            cfg.warnings[1].contains("unknown key \"impure-proc-macro\""),
            "{cfg:?}"
        );
    }

    #[test]
    fn wrong_shapes_fail_loudly() {
        for bad in [
            "[workspace.metadata]\nmitts = 5\n",
            "[workspace.metadata.mitts]\nimpure-proc-macros = \"sqlx\"\n",
            "[workspace.metadata.mitts]\nimpure-proc-macros = [1]\n",
            "[workspace.metadata.mitts]\nimpure-proc-macros = [\"\"]\n",
        ] {
            assert!(
                ProjectConfig::from_manifest_str(bad, Path::new("x")).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn matches_direct_dependencies_by_package_name() {
        let cfg = ProjectConfig::from_manifest_str(
            "[workspace.metadata.mitts]\nimpure-proc-macros = [\"sqlx\", \"my-derive\"]\n",
            Path::new("x"),
        )
        .unwrap();
        assert!(cfg.unit_has_impure_proc_macro(&unit_with_deps(&[
            "serde@1.0.0::lib:serde",
            "sqlx@0.8.0::lib:sqlx",
        ])));
        assert!(cfg.unit_has_impure_proc_macro(&unit_with_deps(&[
            "my_derive@0.1.0::proc-macro:my_derive:host=0a1b",
        ])));
        assert!(!cfg.unit_has_impure_proc_macro(&unit_with_deps(&[
            "serde_derive@1.0.0::proc-macro:serde_derive:host=0a1b",
            "sqlx_core@0.8.0::lib:sqlx_core",
        ])));
        assert!(!ProjectConfig::default()
            .unit_has_impure_proc_macro(&unit_with_deps(&["sqlx@0.8.0::lib:sqlx",])));
    }

    #[test]
    fn load_reads_the_root_manifest_and_follows_edits() {
        let root = tempfile::tempdir().unwrap();
        let manifest = root.path().join("Cargo.toml");
        std::fs::write(&manifest, "[workspace]\nmembers = [\"app\"]\n").unwrap();
        assert!(load(root.path()).unwrap().impure_proc_macros.is_empty());

        std::fs::write(
            &manifest,
            "[workspace]\nmembers = [\"app\"]\n[workspace.metadata.mitts]\nimpure-proc-macros = [\"askama\"]\n",
        )
        .unwrap();
        let cfg = load(root.path()).unwrap();
        assert!(cfg.impure_proc_macros.contains("askama"), "{cfg:?}");

        let missing = root.path().join("nowhere");
        assert_eq!(*load(&missing).unwrap(), ProjectConfig::default());
    }
}
