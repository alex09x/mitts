/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Names rustc and build-script processes for the running-unit display
//! (`core::slow_units`) and finds the codegen settings that make a unit slow.

use super::types::{BuildProgressCallback, BuildProgressEvent, PlannedCompilationUnit};
use crate::core::slow_units::{self, Notice, Sink, UnitGuard};
use std::process::Command;
use std::sync::Arc;

/// Where the watcher's notices go: the progress line gets the running units, and the
/// slow-unit lines are printed above it (never under `--quiet`).
pub fn sink(progress: Option<BuildProgressCallback>, quiet: bool) -> Sink {
    Arc::new(move |notice| match (notice, &progress) {
        (Notice::Running(message), Some(cb)) => cb.0(BuildProgressEvent::Running(message)),
        (Notice::Running(_), None) => {}
        (Notice::Line(_), _) if quiet => {}
        (Notice::Line(line), Some(cb)) => cb.0(BuildProgressEvent::Diagnostic(line)),
        (Notice::Line(line), None) => eprintln!("{line}"),
    })
}

/// Records a rustc process for `unit` until the guard drops.
pub fn track_rustc(unit: &PlannedCompilationUnit, cmd: &Command) -> UnitGuard {
    let host = if unit.for_host { " [host]" } else { "" };
    let args: Vec<String> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    slow_units::start(
        format!(
            "{} {} ({}){host}",
            unit.package_name, unit.target_name, unit.kind
        ),
        rerunnable_command(cmd, &args),
        codegen_hint(&args),
    )
}

/// The command as a user can run it again: the link-timing wrapper (`-C linker=<mitts>`)
/// is shown as the real linker it stands in for.
fn rerunnable_command(cmd: &Command, args: &[String]) -> String {
    let real = cmd
        .get_envs()
        .find(|(key, _)| *key == super::link_timing::REAL_LINKER_ENV)
        .and_then(|(_, value)| value)
        .map(|v| v.to_string_lossy().into_owned());
    let wrapper = std::env::current_exe()
        .ok()
        .map(|exe| format!("linker={}", exe.display()));
    let quote = crate::core::verbosity::shell_quote;
    let mut words = vec![quote(&cmd.get_program().to_string_lossy())];
    for arg in args {
        match (&real, &wrapper) {
            (Some(real), Some(wrapper)) if arg == wrapper => {
                words.push(quote(&format!("linker={real}")))
            }
            _ => words.push(quote(arg)),
        }
    }
    words.join(" ")
}

/// Records the compile (`step` = `compile`) or run (`run`) of `unit`'s build script.
pub fn track_build_script(unit: &PlannedCompilationUnit, cmd: &Command, step: &str) -> UnitGuard {
    slow_units::start(
        format!("{} build script ({step})", unit.package_name),
        crate::core::verbosity::render_command(cmd, false),
        String::new(),
    )
}

/// The `-C` settings among rustc's arguments that decide how long codegen takes:
/// `opt-level=s, codegen-units=1, lto=fat`. A bare `-C lto` is fat LTO.
pub fn codegen_hint(args: &[String]) -> String {
    let mut opt_level = None;
    let mut codegen_units = None;
    let mut lto = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let value = if arg == "-C" || arg == "--codegen" {
            match iter.next() {
                Some(v) => v.as_str(),
                None => break,
            }
        } else if let Some(v) = arg
            .strip_prefix("-C")
            .or_else(|| arg.strip_prefix("--codegen="))
        {
            v
        } else {
            continue;
        };
        let (key, val) = value.split_once('=').unwrap_or((value, ""));
        match key {
            "opt-level" => opt_level = Some(val.to_string()),
            "codegen-units" => codegen_units = Some(val.to_string()),
            "lto" => {
                lto = match val {
                    "" | "y" | "yes" | "on" | "true" | "fat" => Some("fat".to_string()),
                    "thin" => Some("thin".to_string()),
                    _ => None,
                }
            }
            _ => {}
        }
    }
    let mut parts = Vec::new();
    if let Some(v) = opt_level {
        parts.push(format!("opt-level={v}"));
    }
    if let Some(v) = codegen_units {
        parts.push(format!("codegen-units={v}"));
    }
    if let Some(v) = lto {
        parts.push(format!("lto={v}"));
    }
    parts.join(", ")
}

#[cfg(test)]
mod tests {
    use super::codegen_hint;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_hint_names_the_settings_that_slow_codegen_down() {
        assert_eq!(
            codegen_hint(&args(&[
                "--crate-name",
                "melodia_ui",
                "-C",
                "opt-level=s",
                "-C",
                "codegen-units=1",
                "-Cembed-bitcode=no",
                "--edition=2021"
            ])),
            "opt-level=s, codegen-units=1"
        );
        assert_eq!(
            codegen_hint(&args(&["-Copt-level=3", "-Clto", "-C", "codegen-units=1"])),
            "opt-level=3, codegen-units=1, lto=fat"
        );
        assert_eq!(codegen_hint(&args(&["-C", "lto=thin"])), "lto=thin");
        assert_eq!(codegen_hint(&args(&["-C", "lto=off"])), "");
        assert_eq!(
            codegen_hint(&args(&["--codegen=opt-level=2"])),
            "opt-level=2"
        );
        assert_eq!(
            codegen_hint(&args(&["-C", "opt-level=0", "-C", "opt-level=3"])),
            "opt-level=3",
            "the last setting wins, as in rustc"
        );
        assert_eq!(codegen_hint(&args(&["src/main.rs", "-C"])), "");
    }
}
