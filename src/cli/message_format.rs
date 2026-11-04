/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! `--message-format` for `mitts check` and `mitts clippy`.
//!
//! In `json` mode stdout carries one JSON object per line and nothing else:
//! `{"reason":"compiler-message",...}` records, one per compiler diagnostic, and a final
//! `{"reason":"build-finished","success":<bool>}`. Banners, progress and any other text go to
//! stderr, so a consumer can read stdout as a clean JSON stream.

use crate::adapter::{BuildProgressCallback, BuildProgressEvent};
use crate::cli::ui::Ui;
use clap::ValueEnum;
use std::sync::Arc;

/// Prefix every record on stdout starts with; anything else is not part of the stream.
const RECORD_PREFIX: &str = "{\"reason\":";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, ValueEnum)]
pub enum MessageFormat {
    /// Human-readable progress and diagnostics
    #[default]
    Human,
    /// Cargo-compatible JSON lines on stdout
    Json,
}

impl MessageFormat {
    pub fn is_json(self) -> bool {
        self == MessageFormat::Json
    }
}

/// Progress callback for JSON mode. Compiler records go to stdout; every other line, such as
/// output a compiler wrapper printed, goes to stderr.
pub fn json_progress() -> BuildProgressCallback {
    BuildProgressCallback(Arc::new(|event| match event {
        BuildProgressEvent::CompilerMessage(text) | BuildProgressEvent::Diagnostic(text) => {
            for line in text.lines() {
                if line.starts_with(RECORD_PREFIX) {
                    println!("{line}");
                } else {
                    eprintln!("{line}");
                }
            }
        }
        _ => {}
    }))
}

/// Prints the closing record and returns the process exit code. `outcome` is the build's
/// `(success, exit_code)`, or the error that stopped it before any unit ran.
pub fn finish_json(outcome: anyhow::Result<(bool, i32)>) -> i32 {
    match outcome {
        Ok((success, exit_code)) => {
            println!("{{\"reason\":\"build-finished\",\"success\":{success}}}");
            exit_code
        }
        Err(err) => {
            Ui::report_error(&err);
            println!("{{\"reason\":\"build-finished\",\"success\":false}}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_format_is_human() {
        assert_eq!(MessageFormat::default(), MessageFormat::Human);
        assert!(!MessageFormat::Human.is_json());
        assert!(MessageFormat::Json.is_json());
    }

    #[test]
    fn finish_returns_the_build_exit_code() {
        assert_eq!(finish_json(Ok((true, 0))), 0);
        assert_eq!(finish_json(Ok((false, 101))), 101);
    }
}
