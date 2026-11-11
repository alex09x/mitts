/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub mod action;
pub mod action_cache;
pub mod action_manifest;
pub mod auto_gc;
pub mod build_env;
pub mod build_lock;
pub mod build_lock_types;
pub mod build_logger;
pub mod build_report;
pub(crate) mod cache_phase;
pub mod cache_root;
pub mod cas;
pub mod ephemeral_store;
pub mod file_scanner;
pub mod gc;
pub mod hash;
pub mod jobs;
pub mod jobserver;
pub mod lock;
pub mod metrics;
pub mod owned_dir;
pub mod reflink;
pub mod source_store;
pub mod target_index;
pub mod unit_timings;
pub mod verbosity;

pub use action::{Action, ActionKind, ActionResult, BuildPlan, ToolchainInfo};
pub use action_cache::{ActionCache, ActionCacheEntry, CachedOutput};
pub use action_manifest::{ActionManifest, ActionManifestEntry};
pub use build_env::{collect_relevant_env_vars, compute_effective_env_hash};
pub use build_lock::{FastBuildLock, FreshnessCheckArgs, FreshnessResult, RecordStateArgs};
pub use build_logger::BuildLogger;
pub use cas::ContentAddressedStore;
pub use gc::{CacheGc, GcOptions, GcReport};
pub use hash::Sha256;
pub use jobserver::{JobToken, JobserverPool};
pub use lock::FileLock;
pub use reflink::{link_intra_workspace, reflink_or_copy};
pub use source_store::SourceStore;
pub use target_index::{TargetArtifactIndex, TargetArtifactRecord};
