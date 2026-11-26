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

/// The future-incompatibility records of this process's build, `(package id, JSON)`.
static FUTURE_INCOMPAT: std::sync::Mutex<Vec<(String, String)>> = std::sync::Mutex::new(Vec::new());

/// The records collected so far, emptied.
pub fn take_future_incompat_records() -> Vec<(String, String)> {
    std::mem::take(&mut *FUTURE_INCOMPAT.lock().unwrap_or_else(|e| e.into_inner()))
}

/// Takes the future-incompatibility records out of a unit's output (fresh or replayed) and
/// keeps them for the build's report; in text modes the JSON diagnostics a dependency was
/// compiled with are rendered back as text.
fn take_future_incompat(unit: &PlannedCompilationUnit, raw: &str, fmt: DiagnosticFormat) -> String {
    let text_mode = matches!(fmt, DiagnosticFormat::Human | DiagnosticFormat::Short);
    let mut out = String::with_capacity(raw.len());
    for line in raw.lines() {
        if line.contains("\"$message_type\":\"future_incompat\"") {
            FUTURE_INCOMPAT
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push((unit.package_id.clone(), line.to_string()));
            continue;
        }
        if text_mode && !unit.is_workspace {
            if let Some(rendered) = super::pipelining::parse_rustc_rendered_diagnostic(line) {
                out.push_str(&rendered);
                continue;
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    if !raw.ends_with('\n') && out.ends_with('\n') {
        out.pop();
    }
    out
}

/// Records `raw` as compiler output of `unit` and reports it to the progress callback.
/// `failed_command` is the command line of a failed unit; with `-v`, as Cargo shows the
/// process that failed, it prefixes human text. Left out by default (Cargo prints only
/// rustc's diagnostics then) and in JSON and short modes.
pub fn publish(
    ctx: &ExecutionContext,
    unit: &PlannedCompilationUnit,
    raw: &str,
    failed_command: Option<&str>,
) {
    if raw.is_empty() {
        return;
    }
    let raw = &take_future_incompat(unit, raw, ctx.opt.effective_diagnostic_format());
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
            Some(command)
                if fmt == DiagnosticFormat::Human && crate::core::verbosity::level() >= 1 =>
            {
                format!("Command: {command}\n{shown}")
            }
            _ => shown,
        };
        callback.0(BuildProgressEvent::CompilerMessage(message));
    }
}
