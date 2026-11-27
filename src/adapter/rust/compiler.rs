/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::build_script_directives::BuildScriptDirectives;
use super::types::{BuildMode, PlannedCompilationUnit};
use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

pub use super::compiler_resolve::*;

/// Direct `rustc` or `clippy-driver` command invocation builder.
pub struct RustcInvocation<'a> {
    pub unit: &'a PlannedCompilationUnit,
    pub action_key: &'a str,
    pub out_dir: &'a Path,
    pub dest_binary: Option<&'a Path>,
    pub extern_crates: &'a [(String, PathBuf)],
    pub release: bool,
    pub features: &'a [String],
    pub directives: Option<&'a BuildScriptDirectives>,
    pub mode: BuildMode,
    pub extra_compiler_args: &'a [String],
    pub jobserver_auth: Option<&'a str>,
    pub deps_dir: Option<&'a Path>,
    pub target_dir: Option<&'a Path>,
    /// Root of the Mitts cache the build runs against (the parent of its `cas/`);
    /// the default cache root when `None`.
    pub cache_root: Option<&'a Path>,
}

impl<'a> RustcInvocation<'a> {
    /// Executes the direct `rustc` or `clippy-driver` compilation with pipelined metadata callback.
    pub fn execute_pipelined<F>(self, on_metadata: F) -> Result<RustcOutput>
    where
        F: FnOnce(&Path),
    {
        let (mut cmd, clean_crate, is_pipelined_lib) =
            super::compiler_args::build_compiler_command(&self)?;

        let command_line = format!("{:?}", cmd);
        let working_dir = cmd.get_current_dir().map(Path::to_path_buf);
        let verbosity = crate::core::verbosity::level();
        if verbosity >= 1 {
            eprintln!(
                "     Running `{}`",
                crate::core::verbosity::render_command(&cmd, verbosity >= 2)
            );
        }
        let link_report = super::link_timing::report_of(&cmd);
        let running = super::unit_watch::track_rustc(self.unit, &cmd);
        if is_pipelined_lib && super::unit_watch::emits_metadata(&cmd) {
            // rustc writes the `.rmeta` when its front end is done: the rest is codegen.
            // A cdylib, staticlib or dylib unit emits no `.rmeta`, so it has no phases.
            running.track_phases();
        }
        let on_metadata = |path: &Path| {
            running.front_end_done();
            on_metadata(path)
        };

        let (success, mut exit_code, stdout, stderr, mut primary_output, mut metadata_output) =
            if is_pipelined_lib {
                let preserve_json = super::compiler_args::diagnostics::resolve_diagnostic_format(
                    self.extra_compiler_args,
                )
                .is_json();
                let res = super::pipelining::run_pipelined_rustc_with_format(
                    cmd,
                    preserve_json,
                    on_metadata,
                )?;
                (
                    res.success,
                    res.exit_code,
                    res.stdout,
                    res.stderr,
                    res.primary_output,
                    res.metadata_output,
                )
            } else {
                let output = cmd.output().with_context(|| {
                    format!("Failed to execute rustc for unit '{}'", self.unit.unit_id())
                })?;
                let code = output.status.code().unwrap_or(1);
                let success = output.status.success();
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                (success, code, stdout, stderr, None, None)
            };

        if !success {
            exit_code = 101;
        }
        let link_time = link_report.as_deref().and_then(super::link_timing::collect);

        if success {
            let is_bin = self.unit.kind == "bin" || self.unit.kind == "example";
            let is_test = self.unit.kind == "test" || self.unit.kind == "custom-test";
            let is_check_or_clippy_ws = self.unit.is_workspace
                && (self.mode == BuildMode::Check || self.mode == BuildMode::Clippy);
            if (is_bin || is_test) && !is_check_or_clippy_ws {
                // rustc names the executable after the crate, with the target's suffix
                // (`.exe` on Windows, `.wasm` on WebAssembly, `.js` with Emscripten).
                let plain = self.out_dir.join(&clean_crate);
                let bin_path = if plain.is_file() {
                    plain
                } else {
                    super::artifact_kind::EXE_SUFFIXES
                        .iter()
                        .map(|s| self.out_dir.join(format!("{clean_crate}{s}")))
                        .find(|p| p.is_file())
                        .unwrap_or(plain)
                };
                if let Some(dest) = self.dest_binary {
                    if bin_path.is_file() {
                        let _ = dest.parent().map(fs::create_dir_all);
                        if fs::rename(&bin_path, dest).is_err() {
                            let mut roots = vec![self.out_dir];
                            if let Some(td) = self.target_dir {
                                roots.push(td);
                            }
                            let _ =
                                crate::core::link_intra_workspace(&bin_path, dest, &roots, true);
                        }
                    }
                    primary_output = Some(dest.to_path_buf());
                } else if bin_path.is_file() {
                    primary_output = Some(bin_path);
                }
            }

            // Find generated .rlib / .dylib and .rmeta in out_dir if not captured by streaming
            if primary_output.is_none() || metadata_output.is_none() {
                if let Ok(entries) = fs::read_dir(self.out_dir) {
                    // The crate itself, by kind: an rlib, else a dynamic library (`.dll` on
                    // Windows, where a proc-macro or dylib also leaves an import library
                    // `NAME.dll.lib` beside it that rustc cannot load), else a static one.
                    let rank = |p: &Path| match p.extension().and_then(|e| e.to_str()) {
                        Some("rlib") => Some(0),
                        Some("dll" | "dylib" | "so") => Some(1),
                        Some("a") => Some(2),
                        Some("lib") => Some(3),
                        _ => None,
                    };
                    let mut best: Option<(u8, PathBuf)> = None;
                    for p in entries.flatten().map(|e| e.path()) {
                        if p.extension().is_some_and(|e| e == "rmeta") {
                            if metadata_output.is_none() {
                                metadata_output = Some(p);
                            }
                            continue;
                        }
                        if let Some(r) = rank(&p) {
                            if best.as_ref().is_none_or(|(b, _)| r < *b) {
                                best = Some((r, p));
                            }
                        }
                    }
                    if primary_output.is_none() {
                        primary_output = best.map(|(_, p)| p);
                    }
                }
            }

            if self.unit.kind.split(',').any(|kind| kind.trim() == "rlib") {
                if let Ok(entries) = fs::read_dir(self.out_dir) {
                    if let Some(rlib) = entries
                        .flatten()
                        .map(|entry| entry.path())
                        .find(|path| path.extension().is_some_and(|ext| ext == "rlib"))
                    {
                        primary_output = Some(rlib);
                    }
                }
            }

            // In check / clippy mode where no rlib is emitted, primary_output falls back to metadata
            if primary_output.is_none()
                && (self.mode == BuildMode::Check || self.mode == BuildMode::Clippy)
            {
                primary_output = metadata_output.clone();
            }
        }

        Ok(RustcOutput {
            success,
            exit_code,
            stdout,
            stderr,
            command_line,
            working_dir,
            primary_output,
            metadata_output,
            link_time,
        })
    }

    /// Synchronously executes the compiler invocation without pipelining callback.
    pub fn execute(self) -> Result<RustcOutput> {
        self.execute_pipelined(|_| {})
    }
}
