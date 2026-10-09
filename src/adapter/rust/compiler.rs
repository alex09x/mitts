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

        let (success, mut exit_code, stdout, stderr, mut primary_output, mut metadata_output) =
            if is_pipelined_lib {
                let res = super::pipelining::run_pipelined_rustc(cmd, on_metadata)?;
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

        if success {
            let is_bin = self.unit.kind == "bin";
            let is_test = self.unit.kind == "test";
            let is_check_or_clippy_ws = self.unit.is_workspace
                && (self.mode == BuildMode::Check || self.mode == BuildMode::Clippy);
            if (is_bin || is_test) && !is_check_or_clippy_ws {
                let bin_path = self.out_dir.join(&clean_crate);
                if let Some(dest) = self.dest_binary {
                    if bin_path.is_file() {
                        let _ = dest.parent().map(fs::create_dir_all);
                        let _ = crate::core::reflink_or_copy(&bin_path, dest, true);
                    }
                    primary_output = Some(dest.to_path_buf());
                } else if bin_path.is_file() {
                    primary_output = Some(bin_path);
                }
            }

            // Find generated .rlib / .dylib and .rmeta in out_dir if not captured by streaming
            if primary_output.is_none() || metadata_output.is_none() {
                if let Ok(entries) = fs::read_dir(self.out_dir) {
                    for p in entries.flatten().map(|e| e.path()) {
                        match p.extension().and_then(|e| e.to_str()) {
                            Some("rlib" | "dylib" | "so") if primary_output.is_none() => {
                                primary_output = Some(p)
                            }
                            Some("rmeta") if metadata_output.is_none() => metadata_output = Some(p),
                            _ => {}
                        }
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
            primary_output,
            metadata_output,
        })
    }

    /// Synchronously executes the compiler invocation without pipelining callback.
    pub fn execute(self) -> Result<RustcOutput> {
        self.execute_pipelined(|_| {})
    }
}
