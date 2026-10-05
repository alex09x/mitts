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
pub mod cas;
pub mod hash;
pub mod lock;
pub mod reflink;

pub use action::{Action, ActionKind, ActionResult, BuildPlan, ToolchainInfo};
pub use action_cache::{ActionCache, ActionCacheEntry, CachedOutput};
pub use cas::ContentAddressedStore;
pub use hash::Sha256;
pub use lock::FileLock;
pub use reflink::reflink_or_copy;
