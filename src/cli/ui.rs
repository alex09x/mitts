/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use indicatif::{ProgressBar, ProgressStyle};

pub struct Ui;

impl Ui {
    // ANSI color and style constants
    pub const BOLD: &'static str = "\x1b[1m";
    pub const RESET: &'static str = "\x1b[0m";
    pub const CYAN: &'static str = "\x1b[36m";
    pub const GREEN: &'static str = "\x1b[32m";
    pub const YELLOW: &'static str = "\x1b[33m";
    pub const MAGENTA: &'static str = "\x1b[35m";
    pub const DIM: &'static str = "\x1b[2m";
    pub const BOLD_GREEN: &'static str = "\x1b[1;32m";
    pub const BOLD_CYAN: &'static str = "\x1b[1;36m";
    pub const BOLD_YELLOW: &'static str = "\x1b[1;33m";
    pub const RED: &'static str = "\x1b[31m";
    pub const BOLD_RED: &'static str = "\x1b[1;31m";

    /// Prints the main Mitts header banner.
    pub fn header(title: &str) {
        println!(
            "{}{}⚡ Mitts Build{} {}{}{} ──────────────────────────────────────────",
            Self::BOLD_CYAN,
            Self::BOLD,
            Self::RESET,
            Self::DIM,
            title,
            Self::RESET
        );
    }

    /// Prints a top-level stage header: `┌─ 🔍 Stage title`
    pub fn stage_start(icon: &str, title: &str) {
        println!(
            "{}┌─{} {} {}{}{}",
            Self::CYAN,
            Self::RESET,
            icon,
            Self::BOLD,
            title,
            Self::RESET
        );
    }

    /// Prints an intermediate stage transition: `├─ 📦 Stage title`
    pub fn stage_step(icon: &str, title: &str) {
        println!(
            "{}├─{} {} {}{}{}",
            Self::CYAN,
            Self::RESET,
            icon,
            Self::BOLD,
            title,
            Self::RESET
        );
    }

    /// Prints an item under the current stage: `│  ├─ message`
    pub fn stage_item(is_last: bool, badge: &str, msg: &str) {
        let branch = if is_last { "└─" } else { "├─" };
        println!(
            "{}│{}  {}{}{} {} {}",
            Self::CYAN,
            Self::RESET,
            Self::DIM,
            branch,
            Self::RESET,
            badge,
            msg
        );
    }

    /// Prints a cached item badge: `[cached]` in bold green.
    pub fn badge_cached() -> String {
        format!("{}[cached]{}", Self::GREEN, Self::RESET)
    }

    /// Prints a downloaded item badge: `[download]` in bold cyan.
    pub fn badge_download() -> String {
        format!("{}[download]{}", Self::BOLD_CYAN, Self::RESET)
    }

    /// Prints a compiled item badge: `[compile]` in bold yellow.
    pub fn badge_compile() -> String {
        format!("{}[compile]{}", Self::BOLD_YELLOW, Self::RESET)
    }

    /// Prints an Action Cache hit badge: `[AC hit]` in bold green.
    pub fn badge_ac_hit() -> String {
        format!("{}[AC HIT]{}", Self::BOLD_GREEN, Self::RESET)
    }

    /// Prints a failure badge: `[failed]` in bold red.
    pub fn badge_failed() -> String {
        format!("{}[failed]{}", Self::BOLD_RED, Self::RESET)
    }

    /// Prints the final success closing footer: `└─ ✨ Finished in ...`
    pub fn finish(profile: &str, duration_str: &str, extra: &str) {
        println!(
            "{}└─{} ✨ {}{}{} profile completed in {}{}{}{} {}({}){}",
            Self::CYAN,
            Self::RESET,
            Self::BOLD,
            profile,
            Self::RESET,
            Self::GREEN,
            Self::BOLD,
            duration_str,
            Self::RESET,
            Self::DIM,
            extra,
            Self::RESET
        );
        println!();
    }

    /// Prints the final failure closing footer: `└─ ❌ dev profile failed in ...`
    pub fn finish_failed(profile: &str, duration_str: &str, extra: &str) {
        println!(
            "{}└─{} ❌ {}{}{} profile failed in {}{}{}{} {}({}){}",
            Self::CYAN,
            Self::RESET,
            Self::BOLD,
            profile,
            Self::RESET,
            Self::RED,
            Self::BOLD,
            duration_str,
            Self::RESET,
            Self::DIM,
            extra,
            Self::RESET
        );
        println!();
    }

    /// Formats byte sizes into human readable format (KB, MB).
    pub fn format_bytes(bytes: u64) -> String {
        if bytes < 1024 {
            format!("{} B", bytes)
        } else if bytes < 1024 * 1024 {
            format!("{:.1} KB", bytes as f64 / 1024.0)
        } else {
            format!("{:.2} MB", bytes as f64 / (1024.0 * 1024.0))
        }
    }

    /// Creates an animated spinner using indicatif for indeterminate tasks.
    pub fn create_spinner(msg: &str) -> ProgressBar {
        let pb = ProgressBar::new_spinner();
        let style = ProgressStyle::default_spinner()
            .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏")
            .template("{spinner:.cyan.bold} {msg}")
            .unwrap_or_else(|_| ProgressStyle::default_spinner());
        pb.set_style(style);
        pb.set_message(msg.to_string());
        pb.enable_steady_tick(std::time::Duration::from_millis(80));
        pb
    }

    /// Creates an animated build progress bar that starts with indeterminate planning and transitions to a fractional progress bar.
    pub fn create_build_progress(prefix: &str) -> ProgressBar {
        let pb = ProgressBar::new(0);
        let style = ProgressStyle::default_spinner()
            .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏")
            .template("{spinner:.cyan.bold} [{prefix:.bold}] {msg}")
            .unwrap_or_else(|_| ProgressStyle::default_spinner());
        pb.set_style(style);
        pb.set_prefix(prefix.to_string());
        pb.set_message("Planning compilation...".to_string());
        pb.enable_steady_tick(std::time::Duration::from_millis(80));
        pb
    }

    /// Transitions a build progress bar into a determinate bar with exact position and length.
    pub fn set_build_progress_total(pb: &ProgressBar, total: usize, cached: usize) {
        pb.set_length(total as u64);
        pb.set_position(cached as u64);
        let style = ProgressStyle::default_bar()
            .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏")
            .template(
                "{spinner:.cyan.bold} [{prefix:.bold}] [{bar:25.cyan/blue}] {pos}/{len} {msg}",
            )
            .unwrap_or_else(|_| ProgressStyle::default_bar())
            .progress_chars("█▉▊▋▌▍▎▏ ");
        pb.set_style(style);
    }

    /// Creates a styled progress bar using indicatif with Unicode fractional blocks.
    pub fn create_progress_bar(total: u64, prefix: &str) -> ProgressBar {
        let pb = ProgressBar::new(total);
        let style = ProgressStyle::default_bar()
            .template("{prefix:.bold.cyan} {spinner:.green} [{elapsed_precise}] [{bar:30.cyan/blue}] {pos}/{len} ({msg})")
            .unwrap_or_else(|_| ProgressStyle::default_bar())
            .progress_chars("█▉▊▋▌▍▎▏ ");
        pb.set_style(style);
        pb.set_prefix(prefix.to_string());
        pb.enable_steady_tick(std::time::Duration::from_millis(80));
        pb
    }

    /// Renders an error or failure using miette's graphical diagnostic panel.
    pub fn report_error(err: &anyhow::Error) {
        if let Some(diag) = err.downcast_ref::<crate::adapter::RustAdapterError>() {
            let report = miette::Report::new(diag.clone());
            eprintln!("{:?}", report);
        } else {
            let report = miette::miette!("{:#}", err);
            eprintln!("{:?}", report);
        }
    }

    /// Renders a diagnostic message with optional help tip using miette.
    pub fn report_message(msg: &str, help: Option<&str>) {
        if let Some(h) = help {
            let r = miette::miette!(help = h, "{}", msg);
            eprintln!("{:?}", r);
        } else {
            let r = miette::miette!("{}", msg);
            eprintln!("{:?}", r);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ui_format_bytes() {
        assert_eq!(Ui::format_bytes(500), "500 B");
        assert_eq!(Ui::format_bytes(1024), "1.0 KB");
        assert_eq!(Ui::format_bytes(1024 * 1024), "1.00 MB");
    }

    #[test]
    fn test_ui_badges() {
        assert!(Ui::badge_cached().contains("[cached]"));
        assert!(Ui::badge_download().contains("[download]"));
        assert!(Ui::badge_compile().contains("[compile]"));
        assert!(Ui::badge_ac_hit().contains("[AC HIT]"));
    }

    #[test]
    fn test_ui_spinner_and_progress_bar() {
        let spinner = Ui::create_spinner("testing spinner");
        spinner.set_message("spinner updated");
        spinner.finish_and_clear();

        let pb = Ui::create_progress_bar(10, "testing pb");
        pb.inc(5);
        assert_eq!(pb.position(), 5);
        pb.finish_and_clear();
    }
}
