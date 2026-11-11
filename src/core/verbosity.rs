/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! How much a command prints, as Cargo counts it: `-v` once shows every compiler
//! command and every unit taken from the cache, `-vv` adds the environment of those
//! commands and the output of build scripts. `MITTS_VERBOSE=N` sets the level when no
//! `-v` is given (any other non-empty value means 1).

use std::sync::atomic::{AtomicU8, Ordering};

static LEVEL: AtomicU8 = AtomicU8::new(0);

/// Sets the level for this process from the command's `-v` count and `MITTS_VERBOSE`.
pub fn set(flag_count: u8) {
    LEVEL.store(
        resolve(flag_count, std::env::var("MITTS_VERBOSE").ok()),
        Ordering::Relaxed,
    );
}

/// The level from a `-v` count and the value of `MITTS_VERBOSE`; the flag wins.
pub fn resolve(flag_count: u8, env: Option<String>) -> u8 {
    if flag_count > 0 {
        return flag_count;
    }
    match env.as_deref().map(str::trim) {
        None | Some("") => 0,
        Some(value) => value.parse::<u8>().unwrap_or(1),
    }
}

/// The current level: 0 normal, 1 `-v`, 2 or more `-vv`.
pub fn level() -> u8 {
    LEVEL.load(Ordering::Relaxed)
}

/// Quotes a command for display the way a POSIX shell would read it back.
pub fn shell_quote(word: &str) -> String {
    let plain = !word.is_empty()
        && word
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_./=:,+@%^".contains(&b));
    if plain {
        word.to_string()
    } else {
        format!("'{}'", word.replace('\'', r"'\''"))
    }
}

/// `program arg ...` of a command, with `KEY=value` of its explicitly set environment
/// in front when `with_env` is true, as Cargo prints it under `-vv`.
pub fn render_command(cmd: &std::process::Command, with_env: bool) -> String {
    let mut words = Vec::new();
    if with_env {
        let mut envs: Vec<(String, String)> = cmd
            .get_envs()
            .filter_map(|(k, v)| {
                Some((
                    k.to_string_lossy().into_owned(),
                    v?.to_string_lossy().into_owned(),
                ))
            })
            .collect();
        envs.sort();
        for (key, value) in envs {
            words.push(format!("{key}={}", shell_quote(&value)));
        }
    }
    words.push(shell_quote(&cmd.get_program().to_string_lossy()));
    words.extend(cmd.get_args().map(|a| shell_quote(&a.to_string_lossy())));
    words.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flag_count_wins_over_the_variable() {
        assert_eq!(resolve(2, Some("1".into())), 2);
        assert_eq!(resolve(0, Some("2".into())), 2);
        assert_eq!(resolve(0, Some("yes".into())), 1);
        assert_eq!(resolve(0, Some(" ".into())), 0);
        assert_eq!(resolve(0, None), 0);
    }

    #[test]
    fn commands_render_quoted_with_optional_env() {
        let mut cmd = std::process::Command::new("rustc");
        cmd.args(["--crate-name", "demo", "--cfg", "feature=\"std\""]);
        cmd.env("CARGO_PKG_NAME", "demo").env("B", "two words");
        assert_eq!(
            render_command(&cmd, false),
            "rustc --crate-name demo --cfg 'feature=\"std\"'"
        );
        assert_eq!(
            render_command(&cmd, true),
            "B='two words' CARGO_PKG_NAME=demo rustc --crate-name demo --cfg 'feature=\"std\"'"
        );
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
    }
}
