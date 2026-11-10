/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Extra compiler flag forwarding and diagnostic format arguments (`--message-format`).

use crate::adapter::rust::compiler::RustcInvocation;
use crate::adapter::rust::types::{
    BuildMode, DiagnosticFormat, JSON_DIAGNOSTICS_ARG, JSON_DIAG_ANSI_ARG, JSON_DIAG_SHORT_ARG,
    SHORT_DIAGNOSTICS_ARG,
};
use std::path::Path;
use std::process::Command;

pub(crate) fn is_diagnostic_marker_arg(arg: &str) -> bool {
    matches!(
        arg,
        JSON_DIAGNOSTICS_ARG | SHORT_DIAGNOSTICS_ARG | JSON_DIAG_SHORT_ARG | JSON_DIAG_ANSI_ARG
    )
}

pub(crate) fn resolve_diagnostic_format(extra_compiler_args: &[String]) -> DiagnosticFormat {
    if extra_compiler_args
        .iter()
        .any(|arg| arg == SHORT_DIAGNOSTICS_ARG)
    {
        DiagnosticFormat::Short
    } else if extra_compiler_args
        .iter()
        .any(|arg| arg == JSON_DIAG_SHORT_ARG)
    {
        DiagnosticFormat::JsonShort
    } else if extra_compiler_args
        .iter()
        .any(|arg| arg == JSON_DIAG_ANSI_ARG)
    {
        DiagnosticFormat::JsonAnsi
    } else if extra_compiler_args
        .iter()
        .any(|arg| arg == JSON_DIAGNOSTICS_ARG)
    {
        DiagnosticFormat::Json
    } else {
        DiagnosticFormat::Human
    }
}

/// The extra compiler arguments a unit actually gets: no `--workspace`/`--all` or
/// diagnostic markers, no `--target` / coverage instrumentation for host units, and the
/// caller's own flags only for primary targets.
pub(crate) fn forwarded_extra_args(
    inv: &RustcInvocation,
    is_host: bool,
    is_primary_target: bool,
) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < inv.extra_compiler_args.len() {
        let arg = &inv.extra_compiler_args[i];
        let next = inv.extra_compiler_args.get(i + 1).map(String::as_str);
        if arg == "--workspace" || arg == "--all" || is_diagnostic_marker_arg(arg) {
            i += 1;
        } else if arg == "--target" || (arg == "-C" && next == Some("instrument-coverage")) {
            if !is_host {
                out.push(arg.clone());
                out.extend(next.map(String::from));
            }
            i += 2;
        } else if arg.starts_with("--target=") || arg == "-Cinstrument-coverage" {
            if !is_host {
                out.push(arg.clone());
            }
            i += 1;
        } else if is_primary_target {
            out.push(arg.clone());
            i += 1;
        } else if matches!(
            arg.as_str(),
            "-A" | "-W" | "-D" | "-F" | "-C" | "-Z" | "-L" | "-l" | "--cfg" | "--check-cfg"
        ) && next.is_some()
        {
            i += 2;
        } else {
            i += 1;
        }
    }
    out
}

pub(crate) fn append_extra_and_diagnostic_args(
    cmd: &mut Command,
    inv: &RustcInvocation,
    pkg_dir: &Path,
    is_host: bool,
    is_primary_target: bool,
) -> bool {
    cmd.args(forwarded_extra_args(inv, is_host, is_primary_target));

    if crate::adapter::rust::inputs::is_immutable_package_dir(pkg_dir, inv.unit.is_workspace)
        || (!inv.unit.is_workspace && !inv.unit.manifest_path.is_file())
    {
        cmd.args(["--cap-lints", "allow"]);
    }

    let is_pipelined_lib = crate::adapter::rust::planner_units::is_library_kind(&inv.unit.kind)
        && inv.mode != BuildMode::Check
        && inv.mode != BuildMode::Clippy;
    let diag_format = resolve_diagnostic_format(inv.extra_compiler_args);

    if is_pipelined_lib {
        match diag_format {
            DiagnosticFormat::Human | DiagnosticFormat::Json => {
                cmd.args(["--error-format=json", "--json=artifacts"]);
            }
            DiagnosticFormat::Short | DiagnosticFormat::JsonShort => {
                cmd.args(["--error-format=json", "--json=artifacts,diagnostic-short"]);
            }
            DiagnosticFormat::JsonAnsi => {
                cmd.args([
                    "--error-format=json",
                    "--json=artifacts,diagnostic-rendered-ansi",
                ]);
            }
        }
    } else {
        match diag_format {
            DiagnosticFormat::Human => {
                cmd.arg("--color=never");
            }
            DiagnosticFormat::Short => {
                cmd.args([SHORT_DIAGNOSTICS_ARG, "--color=never"]);
            }
            DiagnosticFormat::Json => {
                cmd.arg(JSON_DIAGNOSTICS_ARG);
            }
            DiagnosticFormat::JsonShort => {
                cmd.args([JSON_DIAGNOSTICS_ARG, JSON_DIAG_SHORT_ARG]);
            }
            DiagnosticFormat::JsonAnsi => {
                cmd.args([JSON_DIAGNOSTICS_ARG, JSON_DIAG_ANSI_ARG]);
            }
        }
    }

    is_pipelined_lib
}
