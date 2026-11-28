/*
 * Mitts Build — the linker a rustc unit is given
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! `-C linker` for a unit, placed as Cargo places it: `target.<triple>.linker` (or
//! `CARGO_TARGET_<TRIPLE>_LINKER`) before RUSTFLAGS, so a `-C linker` in RUSTFLAGS wins.
//! A linker named after `--` wins over both and is left alone.
//!
//! When link timing is on, the unit links for the host and the linker is a C compiler
//! driver, Mitts stands in as the linker and runs that driver itself: the configured one,
//! or the one RUSTFLAGS name (that flag is then left out of the arguments, so the
//! wrapper is the only `-C linker`). A `-C linker-flavor` anywhere turns the wrapper off.

use super::super::compiler::RustcInvocation;
use anyhow::Result;
use std::path::Path;
use std::process::Command;

/// Adds the unit's linker to `cmd` and returns the RUSTFLAGS to pass after it.
pub fn apply(
    cmd: &mut Command,
    inv: &RustcInvocation,
    config_start_dir: &Path,
    explicit_target: Option<&str>,
    rustflags: &[String],
    is_check_only: bool,
) -> Result<Vec<String>> {
    if names_linker(inv.extra_compiler_args) {
        return Ok(rustflags.to_vec());
    }
    let is_host = inv.unit.for_host
        || matches!(
            inv.unit.kind.as_str(),
            "proc-macro" | "custom-build" | "build-script"
        );
    // Like Cargo, proc macros and other host units link with `target.<host>.linker`
    // (or CARGO_TARGET_<HOST>_LINKER): without `--target` they also get RUSTFLAGS, and a
    // flag such as `-Clink-arg=--ld-path=wild` needs the driver the user configured.
    let configured = super::super::cargo_config_linker::load_cargo_config_linker(
        config_start_dir,
        if is_host { None } else { explicit_target },
    )?;
    let (from_rustflags, rest) = split_linker(rustflags);
    // The wrapper only stands in for the host's own `cc`-flavored linking: never for a
    // cross target (its default linker or flavor may not be `cc`), never with a linker
    // flavor or a RUSTFLAGS linker spelled in a form not recognised here, and only for
    // units that link.
    let host_target =
        explicit_target.is_none_or(|t| t == super::super::compiler_resolve::resolve_host_triple());
    let flavored = rustflags.iter().any(|f| is_codegen(f, "linker-flavor"))
        || inv
            .extra_compiler_args
            .iter()
            .any(|f| is_codegen(f, "linker-flavor"));
    let unrecognised =
        from_rustflags.is_none() && rustflags.iter().any(|f| is_codegen(f, "linker"));
    let real = from_rustflags.as_deref().or(configured.as_deref());
    let wrapped = !flavored
        && !unrecognised
        && !is_host
        && host_target
        && !is_check_only
        && super::super::pipelining::is_linking_kind(&inv.unit.kind)
        && super::super::link_timing::wrap(cmd, real);
    if wrapped {
        return Ok(if from_rustflags.is_some() {
            rest
        } else {
            rustflags.to_vec()
        });
    }
    if let Some(linker) = configured {
        cmd.args(["-C", &format!("linker={linker}")]);
    }
    Ok(rustflags.to_vec())
}

/// Whether the arguments after `--` name a linker.
fn names_linker(args: &[String]) -> bool {
    args.iter().any(|arg| arg.starts_with("-Clinker="))
        || args
            .windows(2)
            .any(|a| a[0] == "-C" && (a[1] == "linker" || a[1].starts_with("linker=")))
}

/// Whether `flag` mentions the codegen option `name` in any spelling (`-Cname=`,
/// `name=` after a separate `-C`, `--codegen name=`, `--codegen=name=`).
fn is_codegen(flag: &str, name: &str) -> bool {
    let body = flag
        .strip_prefix("--codegen=")
        .or_else(|| flag.strip_prefix("-C"))
        .unwrap_or(flag)
        .trim_start();
    body.strip_prefix(name)
        .is_some_and(|after| after.is_empty() || after.starts_with('='))
}

/// The last `-C linker=X` / `-Clinker=X` in `flags`, and `flags` without every one of
/// them. `None` when there is none, or when one is spelled any other way.
fn split_linker(flags: &[String]) -> (Option<String>, Vec<String>) {
    let mut linker = None;
    let mut rest = Vec::with_capacity(flags.len());
    let mut i = 0;
    while i < flags.len() {
        let flag = &flags[i];
        if let Some(value) = flag.strip_prefix("-Clinker=") {
            linker = Some(value.to_string());
        } else if flag == "-C" && flags.get(i + 1).is_some_and(|f| f.starts_with("linker=")) {
            linker = Some(flags[i + 1]["linker=".len()..].to_string());
            i += 1;
        } else if is_codegen(flag, "linker") {
            return (None, flags.to_vec());
        } else {
            rest.push(flag.clone());
        }
        i += 1;
    }
    (linker.filter(|l| !l.is_empty()), rest)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn the_last_rustflags_linker_is_taken_and_every_one_left_out() {
        let flags = s(&[
            "-C",
            "opt-level=1",
            "-Clinker=gcc",
            "-C",
            "linker=clang",
            "-C",
            "link-arg=-fuse-ld=wild",
        ]);
        let (linker, rest) = split_linker(&flags);
        assert_eq!(linker.as_deref(), Some("clang"));
        assert_eq!(
            rest,
            s(&["-C", "opt-level=1", "-C", "link-arg=-fuse-ld=wild"])
        );
    }

    #[test]
    fn a_linker_in_another_spelling_is_not_taken() {
        for flags in [
            s(&["--codegen", "linker=clang"]),
            s(&["--codegen=linker=clang"]),
            s(&["-C", "linker", "clang"]),
        ] {
            let (linker, rest) = split_linker(&flags);
            assert_eq!(linker, None, "{flags:?}");
            assert_eq!(rest, flags);
        }
    }

    #[test]
    fn linker_flavor_and_link_args_are_not_a_linker() {
        assert!(!is_codegen("linker-flavor=gcc", "linker"));
        assert!(is_codegen("linker-flavor=gcc", "linker-flavor"));
        assert!(is_codegen("-Clinker-flavor=gcc", "linker-flavor"));
        assert!(!is_codegen("link-arg=-fuse-ld=wild", "linker"));
        assert_eq!(split_linker(&s(&["-C", "linker-flavor=gcc"])).0, None);
    }
}
