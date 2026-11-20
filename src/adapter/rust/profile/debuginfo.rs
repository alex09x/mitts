/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Profile `debug` levels, written as rustc's `-C debuginfo=<value>`.
//!
//! Mitts departs from Cargo in one default: a `dev`-based profile (dev, test and custom
//! profiles inheriting dev) that sets no `debug` anywhere gets `line-tables-only`, not
//! `full`. Backtraces keep file and line numbers; the variable and type information only a
//! debugger reads is left out, which keeps test binaries several times smaller. Any
//! explicit `debug` (Cargo.toml, Cargo config, `CARGO_PROFILE_<NAME>_DEBUG`) wins.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DebugInfo {
    None,
    LineDirectivesOnly,
    LineTablesOnly,
    Limited,
    Full,
}

impl DebugInfo {
    /// Whether rustc emits any debug information (Cargo's `DEBUG=true` for build scripts,
    /// and the condition for `split-debuginfo`).
    pub fn is_on(self) -> bool {
        self != DebugInfo::None
    }

    /// A profile `debug` string as Cargo accepts it.
    pub fn parse_str(s: &str) -> Option<Self> {
        match s {
            "false" | "none" | "0" => Some(DebugInfo::None),
            "line-directives-only" => Some(DebugInfo::LineDirectivesOnly),
            "line-tables-only" => Some(DebugInfo::LineTablesOnly),
            "limited" | "1" => Some(DebugInfo::Limited),
            "true" | "full" | "2" => Some(DebugInfo::Full),
            _ => None,
        }
    }

    /// A profile `debug` value: a boolean, 0..=2, or one of the names above.
    pub fn from_toml(val: &toml::Value) -> Option<Self> {
        match val {
            toml::Value::Boolean(true) => Some(DebugInfo::Full),
            toml::Value::Boolean(false) => Some(DebugInfo::None),
            toml::Value::Integer(i) => match (*i).clamp(0, 2) {
                0 => Some(DebugInfo::None),
                1 => Some(DebugInfo::Limited),
                _ => Some(DebugInfo::Full),
            },
            toml::Value::String(s) => Self::parse_str(s),
            _ => None,
        }
    }
}

/// The rustc spelling. The numeric levels stay numbers so keys of profiles that set them
/// do not change.
impl fmt::Display for DebugInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            DebugInfo::None => "0",
            DebugInfo::LineDirectivesOnly => "line-directives-only",
            DebugInfo::LineTablesOnly => "line-tables-only",
            DebugInfo::Limited => "1",
            DebugInfo::Full => "2",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::DebugInfo;

    #[test]
    fn names_reach_rustc_as_cargo_passes_them() {
        for (written, rustc) in [
            ("line-tables-only", "line-tables-only"),
            ("line-directives-only", "line-directives-only"),
            ("limited", "1"),
            ("full", "2"),
            ("none", "0"),
        ] {
            assert_eq!(DebugInfo::parse_str(written).unwrap().to_string(), rustc);
        }
        assert_eq!(DebugInfo::parse_str("everything"), None);
    }

    #[test]
    fn booleans_and_integers() {
        use toml::Value::{Boolean, Integer, String};
        let v = |val: toml::Value| DebugInfo::from_toml(&val);
        assert_eq!(v(Boolean(true)), Some(DebugInfo::Full));
        assert_eq!(v(Boolean(false)), Some(DebugInfo::None));
        assert_eq!(v(Integer(1)), Some(DebugInfo::Limited));
        assert_eq!(v(Integer(2)), Some(DebugInfo::Full));
        assert_eq!(
            v(String("true".into())),
            Some(DebugInfo::Full),
            "env values"
        );
        assert!(!DebugInfo::None.is_on());
        assert!(DebugInfo::LineTablesOnly.is_on());
    }
}
