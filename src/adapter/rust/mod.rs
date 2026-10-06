/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub mod builder;
pub mod dep_info;
pub mod env_macro;
pub mod fetcher;
pub mod inputs;
pub mod key;
pub mod lockfile;
pub mod planner;
pub mod runner;
pub mod types;
pub mod unescape;

pub use env_macro::scan_env_macros;
pub use fetcher::{CrateFetcher, FetchOptions, FetchResult};
pub use key::derive_unit_action_keys;
pub use lockfile::{CargoLock, CargoLockPackage};
pub use types::{
    EffectivePlanExplanation, EnvMacroDiscovery, PlannedCompilationUnit, RustAdapterError,
    RustBuildOptions,
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
        let output = Command::new("rustc")
            .arg("--version")
            .output()
            .context("Failed to invoke rustc --version")?;
        let version_str = String::from_utf8_lossy(&output.stdout).trim().to_string();

        let target_output = Command::new("rustc")
            .args(["-vV"])
            .output()
            .ok();
        let mut target_triple = None;
        if let Some(t_out) = target_output {
            let text = String::from_utf8_lossy(&t_out.stdout);
            for line in text.lines() {
                if let Some(rest) = line.strip_prefix("host: ") {
                    target_triple = Some(rest.trim().to_string());
                }
            }
        }

        Ok(ToolchainInfo {
            name: "rustc".to_string(),
            version: version_str,
            target_triple,
        })
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

    /// Executes the build according to the M2 caching contract.
    pub fn build(opt: &RustBuildOptions) -> Result<(BuildPlan, ActionResult)> {
        builder::build(opt)
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
