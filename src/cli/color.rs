/*
 * Mitts Build — `--color auto|always|never` for Mitts' own output
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Mitts writes its own ANSI styling (banners, stage trees, rendered diagnostics); the CLI
//! modules print through `anstream`, which strips it according to one global choice.
//! `auto` keeps colors only on a terminal and honours `NO_COLOR` / `CLICOLOR_FORCE`.
//! rustc itself always runs with `--color=never`: Mitts renders its diagnostics.

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum ColorWhen {
    Auto,
    Always,
    Never,
}

impl ColorWhen {
    /// `--color`, else Cargo's `CARGO_TERM_COLOR`, else `auto`.
    pub fn resolve(flag: Option<ColorWhen>) -> ColorWhen {
        flag.or_else(|| {
            std::env::var("CARGO_TERM_COLOR").ok().and_then(|value| {
                match value.trim().to_ascii_lowercase().as_str() {
                    "always" => Some(ColorWhen::Always),
                    "never" => Some(ColorWhen::Never),
                    "auto" => Some(ColorWhen::Auto),
                    _ => None,
                }
            })
        })
        .unwrap_or(ColorWhen::Auto)
    }
}

/// Applies the choice to everything Mitts prints, including progress bars.
pub fn apply(when: ColorWhen) {
    let choice = match when {
        ColorWhen::Auto => anstream::ColorChoice::Auto,
        ColorWhen::Always => anstream::ColorChoice::Always,
        ColorWhen::Never => anstream::ColorChoice::Never,
    };
    choice.write_global();
    match when {
        ColorWhen::Auto => {}
        ColorWhen::Always | ColorWhen::Never => {
            let enabled = when == ColorWhen::Always;
            console::set_colors_enabled(enabled);
            console::set_colors_enabled_stderr(enabled);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_flag_wins_over_cargo_term_color() {
        assert_eq!(ColorWhen::resolve(Some(ColorWhen::Never)), ColorWhen::Never);
    }
}
