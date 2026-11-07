/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! How the compiler output of one unit reaches the user: freshly compiled, replayed from the
//! action cache, or produced by Clippy for a build script.

use super::dag_cache::ExecutionContext;
use super::types::{BuildProgressEvent, DiagnosticFormat, PlannedCompilationUnit};

/// The text shown for `raw`. Human and short modes show each diagnostic once per build, like
/// Cargo; JSON modes wrap every diagnostic in a record for `unit` and keep the output order.
pub fn shown_text(ctx: &ExecutionContext, unit: &PlannedCompilationUnit, raw: &str) -> String {
    match ctx.opt.effective_diagnostic_format() {
        DiagnosticFormat::Json | DiagnosticFormat::JsonShort | DiagnosticFormat::JsonAnsi => {
            super::diagnostics_json::wrap(unit, raw)
        }
        DiagnosticFormat::Short => super::diagnostic_dedupe::dedupe_short(raw),
        DiagnosticFormat::Human => super::diagnostic_dedupe::dedupe(raw),
    }
}

/// Records `raw` as compiler output of `unit` and reports it to the progress callback.
/// `failed_command` is the command line of a failed unit; it prefixes human text so the
/// failing invocation is visible, and is left out of JSON and short modes.
pub fn publish(
    ctx: &ExecutionContext,
    unit: &PlannedCompilationUnit,
    raw: &str,
    failed_command: Option<&str>,
) {
    if raw.is_empty() {
        return;
    }
    let shown = shown_text(ctx, unit, raw);
    if shown.is_empty() {
        return;
    }
    ctx.compiler_messages.lock().unwrap().push(shown.clone());
    if let Some(ref callback) = ctx.opt.progress {
        let fmt = ctx.opt.effective_diagnostic_format();
        let message = match failed_command {
            Some(command) if fmt == DiagnosticFormat::Human => {
                format!("Command: {command}\n{shown}")
            }
            _ => shown,
        };
        callback.0(BuildProgressEvent::CompilerMessage(message));
    }
}
