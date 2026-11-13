/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! The lines under a build's result: the toolchain that compiled it and where the time
//! went: the linker, the time line, and the units slower than the slow-unit threshold.
//! `-v` adds the compiler path, why that toolchain was chosen, and the slowest units.

use crate::adapter::rust::{compiler_resolve, link_timing, toolchain_select};
use crate::cli::ui::Ui;
use crate::core::{build_report, metrics, slow_units, verbosity};
use std::time::Duration;

/// The summary lines of the last build, without printing them.
pub fn lines() -> Vec<String> {
    let Some(report) = build_report::get() else {
        return Vec::new();
    };
    let verbose = verbosity::level() >= 1;
    let mut out = Vec::new();
    if let Some(rustc) = &report.rustc {
        let start_dir = std::env::current_dir().unwrap_or_default();
        out.push(toolchain_select::toolchain_line(
            rustc,
            &compiler_resolve::resolve_rustc(),
            verbose,
            &start_dir,
        ));
    }
    let (link, linked_units, linkers) = link_timing::snapshot();
    for linker in &linkers {
        match link_timing::linker_version(linker) {
            Some(version) => out.push(format!("linker: {linker} ({version})")),
            None => out.push(format!("linker: {linker}")),
        }
    }
    if !report.fast_lock {
        let snapshot = metrics::get_metrics_snapshot();
        out.push(build_report::time_line(
            &report,
            Duration::from_nanos(snapshot.9.saturating_sub(report.rustc_nanos_at_start)),
            Some((link, linked_units)),
            Duration::from_nanos(snapshot.6.saturating_sub(report.restore_nanos_at_start)),
        ));
    }
    // The units that crossed the slow-unit threshold; with -v the slowest three anyway.
    let min = if verbose {
        Some(Duration::ZERO)
    } else {
        slow_units::threshold()
    };
    if let Some(line) = min.and_then(slow_units::slowest_line) {
        out.push(line);
    }
    out
}

/// Prints the summary lines as notes of the current stage.
pub fn print() {
    for line in lines() {
        Ui::stage_note(&line);
    }
}

/// The short outcome for the closing line (`2 compiled, 491 from the cache`); `verb` is
/// `compiled` or `checked`.
pub fn outcome(verb: &str) -> Option<String> {
    build_report::get().map(|r| build_report::outcome_text(&r, verb))
}
