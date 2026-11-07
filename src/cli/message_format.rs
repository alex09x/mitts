/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! `--message-format` support across `build`, `check`, `clippy`, `test`, `bench`, and `run`.
//!
//! Matches Cargo's diagnostic formats and comma-separated JSON modifiers:
//! - `human` (default): human-readable progress and multi-line diagnostics.
//! - `short`: single-line `file:line:col: level: message` diagnostics on stderr without banners.
//! - `json`: NDJSON `compiler-message` records + `build-finished` on stdout.
//! - `json-diagnostic-short`: NDJSON stream whose `message.rendered` uses short single-line text.
//! - `json-diagnostic-rendered-ansi`: NDJSON stream whose `message.rendered` carries ANSI escapes.
//! - `json-render-diagnostics`: omits `compiler-message` from stdout, renders `message.rendered`
//!   to stderr instead, and still writes `build-finished` to stdout (combinable with
//!   `json-diagnostic-short` or `json-diagnostic-rendered-ansi`).

use crate::adapter::rust::diagnostic_dedupe;
use crate::adapter::{
    BuildProgressCallback, BuildProgressEvent, DiagnosticFormat, RustAdapter, RustBuildOptions,
};
use crate::cli::ui::Ui;
use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

/// Prefix every JSON record on stdout starts with; anything else goes to stderr.
const RECORD_PREFIX: &str = "{\"reason\":";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MessageFormat {
    #[default]
    Human,
    Short,
    Json,
    JsonDiagnosticShort,
    JsonDiagnosticRenderedAnsi,
    JsonRenderDiagnostics,
    JsonRenderDiagnosticsShort,
    JsonRenderDiagnosticsAnsi,
}

impl MessageFormat {
    pub fn is_json(self) -> bool {
        !matches!(self, Self::Human | Self::Short)
    }

    pub fn is_short(self) -> bool {
        self == Self::Short
    }

    pub fn renders_diagnostics_to_stderr(self) -> bool {
        matches!(
            self,
            Self::JsonRenderDiagnostics
                | Self::JsonRenderDiagnosticsShort
                | Self::JsonRenderDiagnosticsAnsi
        )
    }

    pub fn diagnostic_format(self) -> DiagnosticFormat {
        match self {
            Self::Human => DiagnosticFormat::Human,
            Self::Short => DiagnosticFormat::Short,
            Self::Json | Self::JsonRenderDiagnostics => DiagnosticFormat::Json,
            Self::JsonDiagnosticShort | Self::JsonRenderDiagnosticsShort => {
                DiagnosticFormat::JsonShort
            }
            Self::JsonDiagnosticRenderedAnsi | Self::JsonRenderDiagnosticsAnsi => {
                DiagnosticFormat::JsonAnsi
            }
        }
    }
}

impl fmt::Display for MessageFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Human => "human",
            Self::Short => "short",
            Self::Json => "json",
            Self::JsonDiagnosticShort => "json-diagnostic-short",
            Self::JsonDiagnosticRenderedAnsi => "json-diagnostic-rendered-ansi",
            Self::JsonRenderDiagnostics => "json-render-diagnostics",
            Self::JsonRenderDiagnosticsShort => "json-render-diagnostics,json-diagnostic-short",
            Self::JsonRenderDiagnosticsAnsi => {
                "json-render-diagnostics,json-diagnostic-rendered-ansi"
            }
        };
        f.write_str(name)
    }
}

impl FromStr for MessageFormat {
    type Err = String;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        if raw.trim().is_empty() {
            return Err("`--message-format` requires a format name".to_string());
        }
        let mut saw_human = false;
        let mut saw_short = false;
        let mut saw_json = false;
        let mut saw_json_render = false;
        let mut saw_diag_short = false;
        let mut saw_diag_ansi = false;

        for part in raw.split(',') {
            let token = part.trim();
            let slot = match token {
                "human" => &mut saw_human,
                "short" => &mut saw_short,
                "json" => &mut saw_json,
                "json-render-diagnostics" => &mut saw_json_render,
                "json-diagnostic-short" => &mut saw_diag_short,
                "json-diagnostic-rendered-ansi" => &mut saw_diag_ansi,
                _ => {
                    return Err(format!(
                        "invalid `--message-format` value `{token}` (possible values: human, short, json, json-diagnostic-short, json-diagnostic-rendered-ansi, json-render-diagnostics)"
                    ));
                }
            };
            if *slot {
                return Err(format!(
                    "`--message-format` value `{token}` cannot be specified multiple times"
                ));
            }
            *slot = true;
        }

        let any_json = saw_json || saw_json_render || saw_diag_short || saw_diag_ansi;
        if (saw_human && (saw_short || any_json)) || (saw_short && any_json) {
            return Err(
                "cannot combine `human` or `short` with other `--message-format` values"
                    .to_string(),
            );
        }
        if saw_json && saw_json_render {
            return Err(
                "cannot combine `json` and `json-render-diagnostics` in `--message-format`"
                    .to_string(),
            );
        }
        if saw_diag_short && saw_diag_ansi {
            return Err(
                "cannot combine `json-diagnostic-short` and `json-diagnostic-rendered-ansi`"
                    .to_string(),
            );
        }

        Ok(if saw_human {
            Self::Human
        } else if saw_short {
            Self::Short
        } else if saw_json_render {
            if saw_diag_short {
                Self::JsonRenderDiagnosticsShort
            } else if saw_diag_ansi {
                Self::JsonRenderDiagnosticsAnsi
            } else {
                Self::JsonRenderDiagnostics
            }
        } else if saw_diag_short {
            Self::JsonDiagnosticShort
        } else if saw_diag_ansi {
            Self::JsonDiagnosticRenderedAnsi
        } else {
            Self::Json
        })
    }
}

/// Progress callback for `json` mode (default plain `Json` variant).
pub fn json_progress() -> BuildProgressCallback {
    json_progress_for(MessageFormat::Json)
}

/// Progress callback for any `json*` message format.
pub fn json_progress_for(format: MessageFormat) -> BuildProgressCallback {
    let render_stderr = format.renders_diagnostics_to_stderr();
    let short_dedupe = format == MessageFormat::JsonRenderDiagnosticsShort;
    BuildProgressCallback(Arc::new(move |event| match event {
        BuildProgressEvent::CompilerMessage(text) | BuildProgressEvent::Diagnostic(text) => {
            for line in text.lines() {
                if line.starts_with(RECORD_PREFIX) {
                    if render_stderr {
                        if let Ok(val) = serde_json::from_str::<serde_json::Value>(line) {
                            if val["reason"] == "compiler-message" {
                                if let Some(rendered) = val["message"]["rendered"].as_str() {
                                    let deduped = if short_dedupe {
                                        diagnostic_dedupe::dedupe_short(rendered)
                                    } else {
                                        diagnostic_dedupe::dedupe(rendered)
                                    };
                                    if !deduped.is_empty() {
                                        if deduped.ends_with('\n') {
                                            eprint!("{deduped}");
                                        } else {
                                            eprintln!("{deduped}");
                                        }
                                    }
                                }
                                continue;
                            }
                        }
                        println!("{line}");
                    } else {
                        println!("{line}");
                    }
                } else {
                    eprintln!("{line}");
                }
            }
        }
        _ => {}
    }))
}

/// Progress callback for `--message-format=short`: prints short diagnostics to stderr only.
pub fn short_progress() -> BuildProgressCallback {
    short_progress_tracked(Arc::new(std::sync::atomic::AtomicBool::new(false)))
}

fn short_progress_tracked(emitted: Arc<std::sync::atomic::AtomicBool>) -> BuildProgressCallback {
    BuildProgressCallback(Arc::new(move |event| match event {
        BuildProgressEvent::CompilerMessage(text) | BuildProgressEvent::Diagnostic(text) => {
            if !text.trim().is_empty() {
                emitted.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            if text.ends_with('\n') {
                eprint!("{text}");
            } else {
                eprintln!("{text}");
            }
        }
        _ => {}
    }))
}

/// Emits the closing `build-finished` record on stdout.
pub fn emit_build_finished(success: bool) {
    println!("{{\"reason\":\"build-finished\",\"success\":{success}}}");
}

/// Prints the closing record and returns the process exit code.
pub fn finish_json(outcome: anyhow::Result<(bool, i32)>) -> i32 {
    match outcome {
        Ok((success, exit_code)) => {
            emit_build_finished(success);
            exit_code
        }
        Err(err) => {
            Ui::report_error(&err);
            emit_build_finished(false);
            1
        }
    }
}

/// Executes `opt` when `format` is `short` or any `json*` variant, returning the exit code.
pub fn run_non_human_build(opt: &mut RustBuildOptions, format: MessageFormat) -> i32 {
    opt.diagnostic_format = format.diagnostic_format();
    opt.json_diagnostics = format.is_json();
    if format.is_json() {
        opt.progress = Some(json_progress_for(format));
        let outcome = RustAdapter::build(opt).map(|(_, result)| {
            if !result.success
                && format.renders_diagnostics_to_stderr()
                && !result.stderr.trim().is_empty()
            {
                // Non-rustc failure text (e.g. build.rs compile error) has no JSON lines; surface on stderr.
                if !result
                    .stderr
                    .lines()
                    .any(|l| l.trim_start().starts_with('{'))
                {
                    eprintln!("{}", result.stderr.trim());
                }
            }
            (result.success, result.exit_code)
        });
        finish_json(outcome)
    } else {
        let emitted = Arc::new(std::sync::atomic::AtomicBool::new(false));
        opt.progress = Some(short_progress_tracked(Arc::clone(&emitted)));
        match RustAdapter::build(opt) {
            Ok((_, result)) => {
                if !result.success
                    && !emitted.load(std::sync::atomic::Ordering::Relaxed)
                    && !result.stderr.trim().is_empty()
                {
                    eprintln!("{}", result.stderr.trim());
                }
                result.exit_code
            }
            Err(err) => {
                Ui::report_error(&err);
                1
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_standalone_and_combined_cargo_formats() {
        assert_eq!(MessageFormat::default(), MessageFormat::Human);
        assert_eq!("human".parse::<MessageFormat>(), Ok(MessageFormat::Human));
        assert_eq!("short".parse::<MessageFormat>(), Ok(MessageFormat::Short));
        assert_eq!("json".parse::<MessageFormat>(), Ok(MessageFormat::Json));
        assert_eq!(
            "json-diagnostic-short".parse::<MessageFormat>(),
            Ok(MessageFormat::JsonDiagnosticShort)
        );
        assert_eq!(
            "json,json-diagnostic-short".parse::<MessageFormat>(),
            Ok(MessageFormat::JsonDiagnosticShort)
        );
        assert_eq!(
            "json,json-diagnostic-rendered-ansi".parse::<MessageFormat>(),
            Ok(MessageFormat::JsonDiagnosticRenderedAnsi)
        );
        assert_eq!(
            "json-render-diagnostics,json-diagnostic-short".parse::<MessageFormat>(),
            Ok(MessageFormat::JsonRenderDiagnosticsShort)
        );
        assert_eq!(
            "json-render-diagnostics,json-diagnostic-rendered-ansi".parse::<MessageFormat>(),
            Ok(MessageFormat::JsonRenderDiagnosticsAnsi)
        );
        assert!("human,json".parse::<MessageFormat>().is_err());
        assert!("short,json".parse::<MessageFormat>().is_err());
        assert!("json,json-render-diagnostics"
            .parse::<MessageFormat>()
            .is_err());
        assert!("json-diagnostic-short,json-diagnostic-rendered-ansi"
            .parse::<MessageFormat>()
            .is_err());
        assert!("bogus".parse::<MessageFormat>().is_err());
    }

    #[test]
    fn finish_returns_the_build_exit_code() {
        assert_eq!(finish_json(Ok((true, 0))), 0);
        assert_eq!(finish_json(Ok((false, 101))), 101);
    }
}
