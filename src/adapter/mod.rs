/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub mod c;
pub mod rust;

pub use c::{CAdapter, CBuildOptions, CPackageManifest};
pub use rust::{
    derive_unit_action_keys, BuildMode, BuildProgressCallback, BuildProgressEvent, CargoLock,
    CargoLockPackage, CompilationDag, CrateFetcher, EffectivePlanExplanation, FetchOptions,
    FetchResult, PlannedCompilationUnit, RustAdapter, RustAdapterError, RustBuildOptions,
    RustcInvocation, RustcOutput,
};
