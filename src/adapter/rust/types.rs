/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::core::action::ToolchainInfo;
use miette::Diagnostic;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, thiserror::Error, Diagnostic)]
pub enum RustAdapterError {
    #[error("Manifest not found at {0}")]
    #[diagnostic(
        code(mitts::rust::manifest_not_found),
        help(
            "Ensure Cargo.toml exists at the specified path or run from a valid project directory."
        )
    )]
    ManifestNotFound(PathBuf),

    #[error("Unsupported build option: {0}")]
    #[diagnostic(
        code(mitts::rust::unsupported_option),
        help("Run 'mitts build --help' to see supported options.")
    )]
    UnsupportedOption(String),

    #[error("Cargo execution failed with exit code {0}")]
    #[diagnostic(
        code(mitts::rust::cargo_failed),
        help("Check the compiler diagnostic output above for details.")
    )]
    CargoFailed(i32),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PlannedCompilationUnit {
    #[serde(default)]
    pub package_id: String,
    pub package_name: String,
    pub version: String,
    pub target_name: String,
    pub kind: String,
    pub src_path: PathBuf,
    #[serde(default)]
    pub manifest_path: PathBuf,
    pub cache_eligibility: String,
    #[serde(default)]
    pub is_workspace: bool,
    #[serde(default)]
    pub is_primary: bool,
    #[serde(default)]
    pub env_vars: Vec<String>,
    #[serde(default)]
    pub dependencies: Vec<String>,
    #[serde(default)]
    pub edition: Option<String>,
    #[serde(default)]
    pub features: Vec<String>,
    #[serde(default)]
    pub for_host: bool,
}

impl PlannedCompilationUnit {
    pub fn unit_id(&self) -> String {
        if self.for_host {
            format!("{}::{}:{}:host", self.package_id, self.kind, self.target_name)
        } else {
            format!("{}::{}:{}", self.package_id, self.kind, self.target_name)
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EffectivePlanExplanation {
    pub manifest_path: PathBuf,
    pub target_directory: PathBuf,
    pub profile: String,
    pub toolchain: ToolchainInfo,
    pub units: Vec<PlannedCompilationUnit>,
}

#[derive(Clone)]
pub struct BuildProgressCallback(pub std::sync::Arc<dyn Fn(BuildProgressEvent) + Send + Sync>);

impl std::fmt::Debug for BuildProgressCallback {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "BuildProgressCallback")
    }
}

#[derive(Debug, Clone)]
pub enum BuildProgressEvent {
    InitTotal { total: usize, cached: usize },
    Compiling(String),
    UnitFinished,
    CompilerMessage(String),
    Diagnostic(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum BuildMode {
    #[default]
    Build,
    Check,
    Clippy,
    Test,
}

#[derive(Debug, Clone, Default)]
pub struct RustBuildOptions {
    pub manifest_path: Option<PathBuf>,
    pub package: Option<String>,
    pub release: bool,
    pub target_dir: Option<PathBuf>,
    pub features: Vec<String>,
    pub no_default_features: bool,
    pub all_features: bool,
    pub extra_args: Vec<String>,
    pub no_cache: bool,
    pub cache_dir: Option<PathBuf>,
    pub native: bool,
    pub cargo: bool,
    pub verbose: u8,
    pub mode: BuildMode,
    pub progress: Option<BuildProgressCallback>,
    pub start_dir: Option<PathBuf>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum EnvMacroDiscovery {
    /// Successfully discovered static environment variable names read by env!/option_env!
    Tracked(Vec<String>),
    /// Encountered untracked or dynamic env!/option_env! macro whose variable name cannot be statically resolved
    UntrackedDynamic,
}
