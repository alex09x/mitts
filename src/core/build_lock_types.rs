/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct FileSnapshot {
    pub relative_path: PathBuf,
    pub mtime_secs: u64,
    pub mtime_nanos: u32,
    pub size: u64,
    pub sha256: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FastBuildLock {
    pub version: u32,
    pub profile: String,
    pub package: Option<String>,
    pub features: Vec<String>,
    pub no_default_features: bool,
    pub all_features: bool,
    pub toolchain_version: String,
    #[serde(default)]
    pub options_hash: Option<String>,
    #[serde(default)]
    pub has_complete_inputs: bool,
    #[serde(default)]
    pub rustflags: Option<String>,
    #[serde(default)]
    pub env_hash: Option<String>,
    pub created_at_secs: u64,
    pub action_count: usize,
    pub root_binary: Option<PathBuf>,
    pub cargo_lock: Option<FileSnapshot>,
    pub manifests: Vec<FileSnapshot>,
    pub sources: Vec<FileSnapshot>,
    #[serde(default)]
    pub directories: Vec<FileSnapshot>,
    #[serde(default)]
    pub cargo_configs: Vec<FileSnapshot>,
    #[serde(default)]
    pub clippy_configs: Vec<FileSnapshot>,
    #[serde(default)]
    pub tracked_extra_files: BTreeMap<PathBuf, String>,
    #[serde(default)]
    pub tracked_env_vars: BTreeMap<String, Option<String>>,
    pub outputs: Vec<FileSnapshot>,
    #[serde(default)]
    pub stderr: Option<String>,
}

#[derive(Debug)]
pub enum FreshnessResult {
    Fresh {
        action_count: usize,
        root_binary: Option<PathBuf>,
        stderr: Option<String>,
    },
    Stale(String),
}

#[derive(Debug, Clone)]
pub struct FreshnessCheckArgs<'a> {
    pub project_dir: &'a Path,
    pub profile: &'a str,
    pub package: &'a Option<String>,
    pub features: &'a [String],
    pub no_default_features: bool,
    pub all_features: bool,
    pub toolchain_version: &'a str,
}

#[derive(Debug, Clone)]
pub struct RecordStateArgs<'a> {
    pub project_dir: &'a Path,
    pub profile: &'a str,
    pub package: &'a Option<String>,
    pub features: &'a [String],
    pub no_default_features: bool,
    pub all_features: bool,
    pub toolchain_version: &'a str,
    pub action_count: usize,
    pub root_binary: Option<PathBuf>,
    pub outputs: &'a [PathBuf],
    pub stderr: Option<&'a str>,
}

#[derive(Debug, Clone, Default)]
pub struct RecordStateOptions<'a> {
    pub target_dir: Option<&'a Path>,
    pub options_hash: Option<&'a str>,
    pub extra_package_dirs: &'a [PathBuf],
    pub tracked_extra_files: Option<&'a BTreeMap<PathBuf, String>>,
    pub tracked_env_vars: Option<&'a BTreeMap<String, Option<String>>>,
    pub has_complete_inputs: bool,
}
