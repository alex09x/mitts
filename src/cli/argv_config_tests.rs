/*
 * Mitts Build — tests for command-line changes from Cargo configuration
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::*;

fn os(words: &[&str]) -> Vec<OsString> {
    words.iter().map(OsString::from).collect()
}

#[test]
fn the_subcommand_comes_after_global_options_and_their_values() {
    assert_eq!(subcommand_index(&os(&["mitts", "build"])), Some(1));
    assert_eq!(
        subcommand_index(&os(&["mitts", "--color", "never", "xtask"])),
        Some(3)
    );
    assert_eq!(
        subcommand_index(&os(&["mitts", "-Z", "x", "--timings=json", "b"])),
        Some(4)
    );
    assert_eq!(subcommand_index(&os(&["mitts", "--version"])), None);
}

#[test]
fn config_values_stop_at_the_double_dash() {
    let args = os(&[
        "mitts",
        "build",
        "--config",
        "a.b=1",
        "--config=c.d=2",
        "--",
        "--config",
        "x=1",
    ]);
    assert_eq!(config_values(&args, 2), ["a.b=1", "c.d=2"]);
}

#[test]
fn verbosity_flags_are_recognised_in_every_spelling() {
    let args = os(&["mitts", "build", "-vv"]);
    assert!(has_flag(&args, 2, &["-q", "--quiet", "-v", "--verbose"]));
    let args = os(&["mitts", "run", "--", "-v"]);
    assert!(!has_flag(&args, 2, &["-q", "--quiet", "-v", "--verbose"]));
}

#[test]
fn builtin_commands_are_never_aliases() {
    assert!(is_builtin("build"));
    assert!(!is_builtin("b"));
    assert!(!is_builtin("xtask"));
}
