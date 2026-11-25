/*
 * Mitts Build — when progress bars are drawn, as Cargo's `term.progress.when`
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Cargo draws no progress bar on a dumb terminal (`TERM=dumb`, as Emacs' `M-x compile`
//! sets) or under CI (`CI` or `TF_BUILD` set), unless `term.progress.when` (config or
//! `CARGO_TERM_PROGRESS_WHEN`) says `always`; `never` turns it off everywhere. A spinner
//! on a dumb terminal leaves cursor codes and redrawn lines in the editor's buffer.

use indicatif::{ProgressBar, ProgressDrawTarget};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressWhen {
    Auto,
    Always,
    Never,
}

impl ProgressWhen {
    /// Cargo's `term.progress.when` (`--config`, `CARGO_TERM_PROGRESS_WHEN`, then
    /// `.cargo/config.toml`), else `auto`. An unknown value counts as `auto`.
    pub fn resolve() -> ProgressWhen {
        std::env::current_dir()
            .ok()
            .and_then(|cwd| {
                crate::adapter::rust::cargo_config_value::get(&cwd, "term.progress.when")
                    .ok()
                    .flatten()
            })
            .and_then(|value| value.as_str().ok().map(Self::parse))
            .unwrap_or(ProgressWhen::Auto)
    }

    fn parse(value: &str) -> ProgressWhen {
        match value.trim().to_ascii_lowercase().as_str() {
            "always" => ProgressWhen::Always,
            "never" => ProgressWhen::Never,
            _ => ProgressWhen::Auto,
        }
    }
}

/// Whether a progress bar is drawn, by Cargo's rule. `term` is `TERM`; `ci` is whether
/// `CI` or `TF_BUILD` is set. (Off a terminal indicatif draws nothing anyway.)
pub fn shown(when: ProgressWhen, term: Option<&str>, ci: bool) -> bool {
    match when {
        ProgressWhen::Always => true,
        ProgressWhen::Never => false,
        ProgressWhen::Auto => term != Some("dumb") && !ci,
    }
}

/// Whether this process draws progress, by Cargo's rule and this environment.
pub fn progress_shown() -> bool {
    let term = std::env::var("TERM").ok();
    let ci = std::env::var_os("CI").is_some() || std::env::var_os("TF_BUILD").is_some();
    shown(ProgressWhen::resolve(), term.as_deref(), ci)
}

/// A new progress bar (`len`) or spinner (`None`) that draws only where Cargo would draw
/// progress. Decided before anything is drawn: setting a style or message draws at once.
pub fn new_bar(len: Option<u64>) -> ProgressBar {
    let target = if progress_shown() {
        ProgressDrawTarget::stderr()
    } else {
        ProgressDrawTarget::hidden()
    };
    ProgressBar::with_draw_target(len, target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_hides_on_a_dumb_terminal_and_under_ci() {
        assert!(shown(ProgressWhen::Auto, Some("xterm-256color"), false));
        assert!(shown(ProgressWhen::Auto, None, false));
        assert!(!shown(ProgressWhen::Auto, Some("dumb"), false));
        assert!(!shown(ProgressWhen::Auto, Some("xterm"), true));
    }

    #[test]
    fn always_and_never_win() {
        assert!(shown(ProgressWhen::Always, Some("dumb"), true));
        assert!(!shown(ProgressWhen::Never, Some("xterm"), false));
    }

    #[test]
    fn values_as_cargo_reads_them() {
        assert_eq!(ProgressWhen::parse("always"), ProgressWhen::Always);
        assert_eq!(ProgressWhen::parse(" Never "), ProgressWhen::Never);
        assert_eq!(ProgressWhen::parse("auto"), ProgressWhen::Auto);
        assert_eq!(ProgressWhen::parse("sometimes"), ProgressWhen::Auto);
    }
}
