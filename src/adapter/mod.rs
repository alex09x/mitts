/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub mod rust;

pub use rust::{
    CargoLock, CargoLockPackage, CrateFetcher, EffectivePlanExplanation, FetchOptions, FetchResult,
    PlannedCompilationUnit, RustAdapter, RustAdapterError, RustBuildOptions,
};
