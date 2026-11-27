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

impl<'a> ExecutionContext<'a> {
    /// The same build with another store: the project's own crates use the store of its
    /// target directory (`member_store`).
    pub fn with_store<'b>(
        &'b self,
        cas: &'b ContentAddressedStore,
        ac: &'b ActionCache,
    ) -> ExecutionContext<'b>
    where
        'a: 'b,
    {
        ExecutionContext {
            opt: self.opt,
            toolchain: self.toolchain,
            target_dir: self.target_dir,
            cas,
            ac,
            is_cache_enabled: self.is_cache_enabled,
            completed_artifacts: self.completed_artifacts,
            recorded_actions: self.recorded_actions,
            compiler_messages: self.compiler_messages,
            any_missed: self.any_missed,
            dep_build_metadata: self.dep_build_metadata,
            resolved_keys: self.resolved_keys,
            jobserver: self.jobserver,
            is_first_job: self.is_first_job,
            on_metadata_ready: self.on_metadata_ready,
            compiled_actions: self.compiled_actions,
            cached_actions: self.cached_actions,
            completed_cfgs: self.completed_cfgs,
            completed_env: self.completed_env,
            completed_link_args: self.completed_link_args,
            target_index: self.target_index,
            tracked_extra_files: self.tracked_extra_files,
            tracked_env_vars: self.tracked_env_vars,
        }
    }
}
