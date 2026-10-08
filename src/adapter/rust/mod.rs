/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub mod build_script;
pub mod build_script_directives;
pub mod builder;
pub mod compiler;
pub mod dag;
pub mod dag_cache;
pub mod dag_runner;
pub mod dag_scheduler;
pub mod dag_unit;
pub mod dag_unit_externs;
pub mod dep_info;
pub mod env_macro;
pub mod features;
pub mod fetcher;
pub mod inputs;
pub mod key;
pub mod lockfile;
pub mod manifest;
pub mod manifest_models;
pub mod manifest_tests;
pub mod package_selection;
pub mod pipelining;
pub mod planner;
pub mod resolver;
pub mod source_lookup;
pub mod target_filter;
pub mod sparse_index;
pub mod types;
pub mod unescape;

pub use build_script::execute_build_script;
pub use build_script_directives::BuildScriptDirectives;
pub use compiler::{RustcInvocation, RustcOutput};
pub use dag::CompilationDag;
pub use dag_runner::run_native_dag_build;
pub use dag_scheduler::{LockMode, UnitExecutionStatus, WaveWorkQueue};
pub use env_macro::scan_env_macros;
pub use fetcher::{CrateFetcher, FetchOptions, FetchResult};
pub use key::derive_unit_action_keys;
pub use lockfile::{CargoLock, CargoLockPackage};
pub use resolver::NativeResolver;
pub use sparse_index::SparseIndexClient;
pub use types::{
    BuildMode, BuildProgressCallback, BuildProgressEvent, EffectivePlanExplanation,
    EnvMacroDiscovery, PlannedCompilationUnit, RustAdapterError, RustBuildOptions,
};
pub use unescape::unescape_rust_string;

use crate::core::action::{ActionResult, BuildPlan, ToolchainInfo};
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct RustAdapter;

impl RustAdapter {
    /// Detects current installed rustc toolchain info.
    pub fn detect_toolchain() -> Result<ToolchainInfo> {
        let rustc_path = std::env::var("RUSTC")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                if let Ok(home) = std::env::var("HOME") {
                    let p = PathBuf::from(home).join(".cargo").join("bin").join("rustc");
                    if p.is_file() {
                        return p;
                    }
                }
                PathBuf::from("rustc")
            });

        let cache_dir = std::env::var("HOME")
            .map(|h| PathBuf::from(h).join(".mitts").join("cache"))
            .unwrap_or_else(|_| PathBuf::from(".mitts"));
        let cache_file = cache_dir.join("toolchain_v1.json");

        if let Ok(meta) = std::fs::metadata(&rustc_path) {
            if let Ok(mtime) = meta.modified() {
                let mtime_secs = mtime
                    .duration_since(std::time::SystemTime::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                if let Ok(data) = std::fs::read(&cache_file) {
                    if let Ok((cached_mtime, info)) =
                        serde_json::from_slice::<(u64, ToolchainInfo)>(&data)
                    {
                        if cached_mtime == mtime_secs {
                            return Ok(info);
                        }
                    }
                }
            }
        }

        let output = Command::new("rustc")
            .arg("-vV")
            .output()
            .context("Failed to invoke rustc -vV")?;
        let text = String::from_utf8_lossy(&output.stdout);
        let mut version_str = String::new();
        let mut target_triple = None;

        for line in text.lines() {
            if version_str.is_empty() {
                version_str = line.trim().to_string();
            }
            if let Some(rest) = line.strip_prefix("host: ") {
                target_triple = Some(rest.trim().to_string());
            }
        }

        if version_str.is_empty() {
            version_str = "rustc".to_string();
        }

        let info = ToolchainInfo {
            name: "rustc".to_string(),
            version: version_str,
            target_triple,
        };

        if let Ok(meta) = std::fs::metadata(&rustc_path) {
            if let Ok(mtime) = meta.modified() {
                let mtime_secs = mtime
                    .duration_since(std::time::SystemTime::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                let _ = std::fs::create_dir_all(&cache_dir);
                let _ = std::fs::write(
                    &cache_file,
                    serde_json::to_vec(&(mtime_secs, &info)).unwrap_or_default(),
                );
            }
        }

        Ok(info)
    }

    /// Resolves the canonical manifest path for the target build.
    pub fn resolve_manifest(opt: &RustBuildOptions) -> Result<PathBuf> {
        if let Some(ref p) = opt.manifest_path {
            let path = if p.is_absolute() {
                p.clone()
            } else {
                std::env::current_dir()?.join(p)
            };
            if !path.exists() {
                return Err(RustAdapterError::ManifestNotFound(path).into());
            }
            Ok(path)
        } else {
            let default_path = std::env::current_dir()?.join("Cargo.toml");
            if !default_path.exists() {
                return Err(RustAdapterError::ManifestNotFound(default_path).into());
            }
            Ok(default_path)
        }
    }

    /// Resolves the dedicated isolated Mitts target directory for the workspace.
    pub fn resolve_target_dir(manifest_path: &Path, opt: &RustBuildOptions) -> Result<PathBuf> {
        if let Some(ref td) = opt.target_dir {
            Ok(td.clone())
        } else {
            let project_dir = manifest_path
                .parent()
                .context("Manifest path has no parent directory")?;
            Ok(project_dir.join(".mitts").join("target"))
        }
    }

    /// Derives the effective build plan and compilation units from Cargo metadata.
    pub fn explain_plan(opt: &RustBuildOptions) -> Result<EffectivePlanExplanation> {
        planner::explain_plan(opt)
    }

    /// Derives deterministic Action Keys for each compilation unit in the plan.
    pub fn derive_unit_action_keys(
        opt: &RustBuildOptions,
        explanation: &EffectivePlanExplanation,
    ) -> Result<HashMap<String, String>> {
        key::derive_unit_action_keys(opt, explanation)
    }

    /// Executes the build according to the M2/M4 caching contract.
    pub fn build(opt: &RustBuildOptions) -> Result<(BuildPlan, ActionResult)> {
        builder::build(opt)
    }

    /// Executes native parallel DAG compilation directly invoking rustc (Milestone M4).
    pub fn build_native(opt: &RustBuildOptions) -> Result<(BuildPlan, ActionResult)> {
        let explanation = Self::explain_plan(opt)?;
        let unit_keys = Self::derive_unit_action_keys(opt, &explanation)?;
        dag_runner::run_native_dag_build(opt, &explanation, &unit_keys)
    }

    /// Resolves or validates the project's Cargo.lock according to the two-phase contract.
    pub fn resolve_lockfile(manifest_path: &Path, offline: bool) -> Result<lockfile::CargoLock> {
        lockfile::CargoLock::resolve_or_sync(manifest_path, offline)
    }

    /// Fetches all dependencies in Cargo.lock into the global SourceStore.
    pub fn fetch(
        manifest_path: &Path,
        store: &crate::core::source_store::SourceStore,
        opt: &fetcher::FetchOptions,
    ) -> Result<fetcher::FetchResult> {
        let lock = Self::resolve_lockfile(manifest_path, opt.offline)?;
        let fetcher = fetcher::CrateFetcher::new(store.clone());
        fetcher.fetch_all(&lock, opt)
    }
}
