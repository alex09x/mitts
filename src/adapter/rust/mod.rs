/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub(crate) mod build_deps;
pub mod build_script;
pub mod build_script_cas;
pub mod build_script_directives;
pub mod build_script_env;
pub mod build_script_keys;
pub mod build_script_purity;
pub mod builder;
pub mod cargo_config;
pub mod cargo_config_cli;
pub mod cargo_config_linker;
pub mod cargo_config_target_dir;
pub mod compiler;
pub mod compiler_args;
mod compiler_remap;
pub mod compiler_resolve;
pub mod compiler_wrappers;
pub mod credential_provider;
pub mod dag;
pub mod dag_cache;
pub(crate) mod dag_retract;
pub mod dag_runner;
pub mod dag_scheduler;
pub mod dag_unit;
pub mod dag_unit_build_script;
pub mod dag_unit_externs;
pub mod dag_unit_publish;
pub mod dag_worker;
pub mod dep_info;
pub mod dependency_check;
pub mod diagnostic_dedupe;
pub mod diagnostics_json;
pub mod doctest;
pub mod env_macro;
pub mod env_macro_cache;
pub mod env_macro_scope;
pub mod features;
pub mod features_helpers;
pub mod fetcher;
pub mod git_resolve;
pub mod git_source;
pub mod inputs;
pub mod key;
pub mod key_flags;
pub mod lockfile;
pub mod manifest;
pub mod manifest_digest;
pub mod manifest_inherit;
pub mod manifest_models;
pub mod manifest_snapshot;
pub mod manifest_tests;
pub mod manifest_workspace;
pub mod net_retry;
pub mod package_selection;
pub mod patch_resolve;
pub mod pipelining;
pub mod planner;
pub mod planner_deps;
mod planner_paths;
mod planner_roots;
pub mod planner_units;
pub mod profile;
pub mod project_config;
pub mod registry_config;
pub mod resolve_graph;
pub mod resolver;
pub mod single_flight;
pub mod source_lookup;
pub mod sparse_index;
pub mod target_filter;
mod target_validation;
pub mod test_selection;
pub mod toolchain_probes;
pub mod types;
pub mod unescape;
pub mod unit_env;
pub mod unit_inputs;
pub mod unit_output;
pub mod vendor_source;
pub(crate) mod wrapper_outputs;

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
    BuildMode, BuildProgressCallback, BuildProgressEvent, DiagnosticFormat,
    EffectivePlanExplanation, EnvMacroDiscovery, PlannedCompilationUnit, RustAdapterError,
    RustBuildOptions,
};
pub use unescape::unescape_rust_string;

use crate::core::action::{ActionResult, BuildPlan, ToolchainInfo};
use anyhow::{Context, Result};
use manifest::CargoToml;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub struct RustAdapter;

impl RustAdapter {
    /// Detects current installed rustc toolchain info using the active resolved compiler (`resolve_rustc()`).
    pub fn detect_toolchain() -> Result<ToolchainInfo> {
        Ok(compiler_resolve::active_toolchain()?.toolchain)
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
            // `--manifest-path sub/../Cargo.toml` and `./Cargo.toml` name the same project:
            // directories derived from this path reach unit sources and keys.
            Ok(planner_paths::normalize_lexically(&path))
        } else {
            // Like Cargo: the nearest `Cargo.toml` in the current directory or any parent.
            let cwd = std::env::current_dir()?;
            cwd.ancestors()
                .map(|dir| dir.join("Cargo.toml"))
                .find(|candidate| candidate.is_file())
                .ok_or_else(|| RustAdapterError::ManifestNotFound(cwd.join("Cargo.toml")).into())
        }
    }

    /// Resolves the Rust target root, using Cargo's standard target directory by default.
    pub fn resolve_target_dir(manifest_path: &Path, opt: &RustBuildOptions) -> Result<PathBuf> {
        let target_dir = Self::resolve_target_dir_without_tags(manifest_path, opt)?;

        // Ensure target directory has CACHEDIR.TAG and .gitignore so IDEs/tools treat it as build cache
        if opt.target_dir.is_none() {
            let _ = Self::ensure_target_dir_tags(&target_dir);
        }
        Ok(target_dir)
    }

    /// Cleanup resolves paths without creating an otherwise absent output root.
    pub(crate) fn resolve_target_dir_without_tags(
        manifest_path: &Path,
        opt: &RustBuildOptions,
    ) -> Result<PathBuf> {
        let mut target_dir = if let Some(ref td) = opt.target_dir {
            td.clone()
        } else if let Some(cargo_td) = crate::core::build_env::env_target_dir() {
            cargo_td
        } else {
            let project_dir = manifest_path
                .parent()
                .context("Manifest path has no parent directory")?;
            // Like Cargo: a workspace member builds into the workspace root's `target`.
            let workspace_dir = CargoToml::from_file(manifest_path)
                .ok()
                .and_then(|manifest| {
                    CargoToml::find_workspace_root(manifest_path, &manifest)
                        .ok()
                        .flatten()
                })
                .and_then(|(root_manifest, _)| root_manifest.parent().map(Path::to_path_buf))
                .unwrap_or_else(|| project_dir.to_path_buf());
            // `build.target-dir` from .cargo/config.toml, then `<workspace>/target`.
            let config_start = cargo_config::resolve_config_start_dir_with_opt(
                &workspace_dir,
                opt.start_dir.as_deref(),
            );
            cargo_config_target_dir::load_build_target_dir(&config_start)?
                .unwrap_or_else(|| workspace_dir.join("target"))
        };

        if let Some(target) = key_flags::resolve_explicit_target(opt) {
            target_dir.push(target);
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
    /// `cache_dir` relocates the sparse-index cache used when the lockfile has to be synced.
    pub fn resolve_lockfile(
        manifest_path: &Path,
        offline: bool,
        cache_dir: Option<&Path>,
    ) -> Result<lockfile::CargoLock> {
        Self::resolve_lockfile_with_mode(manifest_path, offline, false, cache_dir)
    }

    pub fn resolve_lockfile_with_mode(
        manifest_path: &Path,
        offline: bool,
        locked: bool,
        cache_dir: Option<&Path>,
    ) -> Result<lockfile::CargoLock> {
        lockfile::CargoLock::resolve_or_sync_with_mode(manifest_path, offline, locked, cache_dir)
    }

    /// Fetches all dependencies in Cargo.lock into `store`. `cache_dir` is the cache root
    /// whose `index/` caches sparse-index entries when the lockfile has to be synced.
    pub fn fetch(
        manifest_path: &Path,
        store: &crate::core::source_store::SourceStore,
        opt: &fetcher::FetchOptions,
        cache_dir: Option<&Path>,
    ) -> Result<fetcher::FetchResult> {
        let effective_opt = fetcher::FetchOptions {
            offline: opt.offline || opt.frozen,
            locked: opt.locked || opt.frozen,
            frozen: opt.frozen,
        };
        let lock = Self::resolve_lockfile_with_mode(
            manifest_path,
            effective_opt.offline,
            effective_opt.locked,
            cache_dir,
        )?;
        let project_dir = manifest_path.parent().unwrap_or(Path::new("."));
        let config = registry_config::RegistryConfig::load(project_dir)?;
        let fetcher = fetcher::CrateFetcher::with_config(store.clone(), config);
        fetcher.fetch_all(&lock, &effective_opt)
    }
}
