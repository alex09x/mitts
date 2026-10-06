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
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum RustAdapterError {
    #[error("Manifest not found at {0}")]
    ManifestNotFound(PathBuf),
    #[error("Unsupported build option: {0}")]
    UnsupportedOption(String),
    #[error("Cargo execution failed with exit code {0}")]
    CargoFailed(i32),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
    pub env_vars: Vec<String>,
    #[serde(default)]
    pub dependencies: Vec<String>,
    #[serde(default)]
    pub edition: Option<String>,
}

impl PlannedCompilationUnit {
    pub fn unit_id(&self) -> String {
        format!("{}::{}:{}", self.package_id, self.kind, self.target_name)
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
}

#[derive(Debug, PartialEq, Eq)]
pub enum EnvMacroDiscovery {
    /// Successfully discovered static environment variable names read by env!/option_env!
    Tracked(Vec<String>),
    /// Encountered untracked or dynamic env!/option_env! macro whose variable name cannot be statically resolved
    UntrackedDynamic,
}

/// JSON message shapes emitted by Cargo with `--message-format=json`.
#[derive(Debug, Deserialize)]
#[serde(tag = "reason")]
#[allow(dead_code)]
pub(crate) enum CargoMessage {
    #[serde(rename = "compiler-artifact")]
    CompilerArtifact {
        package_id: String,
        target: CargoTarget,
        profile: CargoProfile,
        features: Vec<String>,
        filenames: Vec<String>,
        executable: Option<String>,
        fresh: bool,
    },
    #[serde(rename = "build-script-executed")]
    BuildScriptExecuted {
        package_id: String,
        linked_libs: Vec<String>,
        linked_paths: Vec<String>,
    },
    #[serde(rename = "compiler-message")]
    CompilerMessage {
        rendered: Option<String>,
    },
    #[serde(rename = "build-finished")]
    BuildFinished {
        success: bool,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub(crate) struct CargoTarget {
    pub name: String,
    pub kind: Vec<String>,
    pub src_path: String,
    pub edition: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub(crate) struct CargoProfile {
    pub opt_level: String,
    pub debuginfo: Option<serde_json::Value>,
    pub test: bool,
}

#[derive(Debug, Deserialize)]
pub(crate) struct MetadataOutput {
    pub packages: Vec<MetaPackage>,
    pub workspace_members: Vec<String>,
    #[serde(default)]
    pub workspace_default_members: Vec<String>,
    pub resolve: Option<MetaResolve>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct MetaPackage {
    pub id: String,
    pub name: String,
    pub version: String,
    pub manifest_path: String,
    #[allow(dead_code)]
    pub source: Option<String>,
    pub targets: Vec<MetaTarget>,
    #[serde(default)]
    pub edition: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct MetaTarget {
    pub name: String,
    pub kind: Vec<String>,
    pub src_path: String,
    #[serde(default)]
    pub edition: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct MetaResolve {
    pub nodes: Vec<MetaResolveNode>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct MetaResolveNode {
    pub id: String,
    pub dependencies: Vec<String>,
}
