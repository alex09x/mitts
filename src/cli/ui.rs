/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

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

    /// Prints the final success closing footer: `└─ ✨ Finished in ...`
    pub fn finish(profile: &str, duration_str: &str, extra: &str) {
        println!(
            "{}└─{} ✨ {}{}{} profile {} in {}{}{} {}({}){}",
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
}
