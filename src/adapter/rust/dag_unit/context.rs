/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::adapter::rust::types::RustBuildOptions;
use crate::core::action::{Action, ToolchainInfo};
use crate::core::action_cache::ActionCache;
use crate::core::cas::ContentAddressedStore;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub type MetadataReadyCallback<'a> = &'a (dyn Fn(&str, &Path) + Send + Sync);
pub type CompletedCfgsMap = Arc<Mutex<HashMap<String, Vec<String>>>>;
pub type CompletedEnvMap = Arc<Mutex<HashMap<String, HashMap<String, String>>>>;
pub type CompletedLinkArgsMap = Arc<Mutex<HashMap<String, Vec<String>>>>;
pub type TrackedExtraFilesMap = Arc<Mutex<std::collections::BTreeMap<PathBuf, String>>>;
pub type TrackedEnvVarsMap = Arc<Mutex<std::collections::BTreeMap<String, Option<String>>>>;

pub struct ExecutionContext<'a> {
    pub opt: &'a RustBuildOptions,
    pub toolchain: &'a ToolchainInfo,
    pub target_dir: &'a Path,
    pub cas: &'a ContentAddressedStore,
    pub ac: &'a ActionCache,
    pub is_cache_enabled: bool,
    pub completed_artifacts: &'a Arc<Mutex<HashMap<String, PathBuf>>>,
    pub recorded_actions: &'a Arc<Mutex<Vec<Action>>>,
    pub compiler_messages: &'a Arc<Mutex<Vec<String>>>,
    pub any_missed: &'a Arc<Mutex<bool>>,
    pub dep_build_metadata: &'a Arc<Mutex<HashMap<String, HashMap<String, String>>>>,
    pub resolved_keys: Option<&'a Arc<Mutex<HashMap<String, String>>>>,
    pub jobserver: Option<&'a Arc<crate::core::JobserverPool>>,
    pub is_first_job: Option<&'a Arc<std::sync::atomic::AtomicBool>>,
    pub on_metadata_ready: Option<MetadataReadyCallback<'a>>,
    pub compiled_actions: Option<&'a Arc<std::sync::atomic::AtomicUsize>>,
    pub cached_actions: Option<&'a Arc<std::sync::atomic::AtomicUsize>>,
    pub completed_cfgs: Option<&'a CompletedCfgsMap>,
    pub completed_env: Option<&'a CompletedEnvMap>,
    pub completed_link_args: Option<&'a CompletedLinkArgsMap>,
    pub target_index: Option<&'a Arc<Mutex<crate::core::TargetArtifactIndex>>>,
    pub tracked_extra_files: Option<&'a TrackedExtraFilesMap>,
    pub tracked_env_vars: Option<&'a TrackedEnvVarsMap>,
}
