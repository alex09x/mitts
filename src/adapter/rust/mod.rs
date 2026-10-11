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
pub mod build_script_env;
pub mod build_script_keys;
pub mod build_script_purity;
pub mod builder;
pub mod cargo_config;
pub mod compiler;
pub mod compiler_args;
pub mod compiler_resolve;
pub mod dag;
pub mod dag_cache;
pub mod dag_runner;
pub mod dag_scheduler;
pub mod dag_unit;
pub mod dag_unit_build_script;
pub mod dag_unit_externs;
pub mod dag_unit_publish;
pub mod dep_info;
pub mod dependency_check;
pub mod doctest;
pub mod env_macro;
pub mod features;
pub mod features_helpers;
pub mod fetcher;
pub mod inputs;
pub mod key;
pub mod key_flags;
pub mod lockfile;
pub mod manifest;
pub mod manifest_inherit;
pub mod manifest_models;
pub mod manifest_tests;
pub mod manifest_workspace;
pub mod package_selection;
pub mod pipelining;
pub mod planner;
pub mod planner_deps;
pub mod planner_units;
pub mod profile;
pub mod resolver;
pub mod source_lookup;
pub mod sparse_index;
pub mod target_filter;
pub mod target_incompatible;
pub mod test_selection;
pub mod types;
pub mod unescape;
pub mod unit_env;
pub mod unit_inputs;

pub use build_script::execute_build_script;
pub use build_script_directives::BuildScriptDirectives;
pub use compiler::{RustcInvocation, RustcOutput};
pub use dag::CompilationDag;
pub use dag_runner::run_native_dag_build;
pub use dag_scheduler::{LockMode, UnitExecutionStatus, WaveWorkQueue};
pub use env_macro::scan_env_macros;
pub use fetcher::{CrateFetcher, FetchOptions, FetchResult};
pub use key::{derive_unit_action_key_context, derive_unit_action_keys, UnitKeyDeriver};
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

    /// Resolves the dedicated isolated Mitts target directory for the workspace (target/mitts).
    pub fn resolve_target_dir(manifest_path: &Path, opt: &RustBuildOptions) -> Result<PathBuf> {
        let target_dir = if let Some(ref td) = opt.target_dir {
            td.clone()
        } else if let Ok(cargo_td) = std::env::var("CARGO_TARGET_DIR") {
            PathBuf::from(cargo_td).join("mitts")
        } else {
            let project_dir = manifest_path
                .parent()
                .context("Manifest path has no parent directory")?;
            project_dir.join("target").join("mitts")
        };

        // Ensure target directory has CACHEDIR.TAG and .gitignore so IDEs/tools treat it as build cache
        if let Some(parent) = target_dir.parent() {
            let _ = Self::ensure_target_dir_tags(parent);
        }

        Ok(target_dir)
    }

    /// Ensures standard CACHEDIR.TAG and .gitignore exist in the target directory.
    fn ensure_target_dir_tags(target_dir: &Path) -> std::io::Result<()> {
        let tag_file = target_dir.join("CACHEDIR.TAG");
        if !tag_file.exists() {
            let _ = std::fs::create_dir_all(target_dir);
            let content = "Signature: 8a477f597d28d172789f06886806bc55\n# This file is a cache directory tag created by Mitts.\n# For information about cache directory tags, see:\n#\thttp://www.brynosaurus.com/cachedir/\n";
            let _ = std::fs::write(&tag_file, content);
        }
        let gitignore = target_dir.join(".gitignore");
        if !gitignore.exists() {
            let _ = std::fs::create_dir_all(target_dir);
            let _ = std::fs::write(&gitignore, "*\n");
        }
        Ok(())
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
        let deriver = key::derive_unit_action_key_context(opt, &explanation)?;
        dag_runner::run_native_dag_build(opt, &explanation, &deriver)
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
