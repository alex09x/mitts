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
use super::types::{BuildProgressEvent, PlannedCompilationUnit};

/// The text shown for `raw`. Human mode shows each diagnostic once per build, like Cargo; JSON
/// mode wraps every diagnostic in a record for `unit` and keeps the order of the output.
pub fn shown_text(ctx: &ExecutionContext, unit: &PlannedCompilationUnit, raw: &str) -> String {
    if ctx.opt.json_diagnostics {
        super::diagnostics_json::wrap(unit, raw)
    } else {
        super::diagnostic_dedupe::dedupe(raw)
    }
}

/// Records `raw` as compiler output of `unit` and reports it to the progress callback.
/// `failed_command` is the command line of a failed unit; it prefixes the human text so the
/// failing invocation is visible, and is left out of JSON mode, where stdout is a clean stream.
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
        let message = match failed_command {
            Some(command) if !ctx.opt.json_diagnostics => format!("Command: {command}\n{shown}"),
            _ => shown,
        };
        callback.0(BuildProgressEvent::CompilerMessage(message));
    }
}
