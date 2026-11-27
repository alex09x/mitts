/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Options of the commands that compile (`build`, `check`, `test`, `run`, `doc`, `fix`,
//! `install`, ...), as in Cargo, where they are options of those commands and not global:
//! `mitts fmt --help` or `mitts gc --help` does not list them.

use clap::Args;

#[derive(Args, Debug, Clone, Default)]
pub struct BuildFlags {
    /// Write a per-unit timing report to target/cargo-timings (html by default; html,json)
    #[arg(long, value_name = "FMTS", num_args = 0..=1, require_equals = true, default_missing_value = "html")]
    pub timings: Option<String>,

    /// Report every compiler or build-script process still running after DURATION (30s, 10m, 1h; off = never; default 5m)
    #[arg(long, value_name = "DURATION")]
    pub slow_units: Option<String>,

    /// Build even when a package's `rust-version` is newer than the rustc in use
    #[arg(long)]
    pub ignore_rust_version: bool,

    /// Print a report of future-incompatibility warnings at the end of the build
    #[arg(long)]
    pub future_incompat_report: bool,
}
