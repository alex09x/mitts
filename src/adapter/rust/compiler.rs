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
use std::process::Command;

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
}

impl<'a> RustcInvocation<'a> {
    /// Executes the direct `rustc` or `clippy-driver` compilation with pipelined metadata callback.
    pub fn execute_pipelined<F>(self, on_metadata: F) -> Result<RustcOutput>
    where
        F: FnOnce(&Path),
    {
        let clean_crate = self.unit.target_name.replace('-', "_");
        let edition = self.unit.edition.as_deref().unwrap_or("2015");
        let short_key = if self.action_key.len() > 16 {
            &self.action_key[..16]
        } else {
            self.action_key
        };

        let compiler_bin = if self.mode == BuildMode::Clippy && self.unit.is_workspace {
            resolve_clippy_driver()
        } else {
            resolve_rustc()
        };

        let mut cmd = Command::new(&compiler_bin);
        cmd.arg("--crate-name").arg(&clean_crate);

        let actual_src = if self.unit.src_path.is_file() {
            self.unit.src_path.clone()
        } else {
            let cache_base = std::env::var("MITTS_CACHE_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|_| {
                    std::env::var("HOME")
                        .map(|h| PathBuf::from(h).join(".cache").join("mitts"))
                        .unwrap_or_else(|_| PathBuf::from(".mitts_cache"))
                });
            let store_src = cache_base
                .join("sources")
                .join(format!("{}-{}", self.unit.package_name, self.unit.version))
                .join("src")
                .join(if self.unit.kind == "bin" {
                    "main.rs"
                } else {
                    "lib.rs"
                });
            if store_src.is_file() {
                store_src
            } else {
                self.unit.src_path.clone()
            }
        };
        cmd.arg(&actual_src);
        cmd.arg("--edition").arg(edition);

        let is_bin = self.unit.kind == "bin";
        let is_test = self.unit.kind == "test";
        let is_proc_macro = self.unit.kind == "proc-macro";

        if is_test {
            cmd.arg("--test");
            cmd.arg("--emit=dep-info,link");
            cmd.arg("--extern").arg("proc_macro");
            cmd.arg("--out-dir").arg(self.out_dir);
        } else if is_proc_macro {
            cmd.arg("--crate-type").arg("proc-macro");
            cmd.arg("--extern").arg("proc_macro");
            cmd.arg("-C").arg("prefer-dynamic");
            cmd.arg("--emit=dep-info,link");
            cmd.arg("-C").arg(format!("metadata={}", short_key));
            cmd.arg("-C").arg(format!("extra-filename=-{}", short_key));
            cmd.arg("--out-dir").arg(self.out_dir);
        } else if (self.mode == BuildMode::Check || self.mode == BuildMode::Clippy)
            && self.unit.is_workspace
        {
            if is_bin {
                cmd.arg("--crate-type").arg("bin");
            } else {
                cmd.arg("--crate-type").arg("rlib");
            }
            cmd.arg("--emit=dep-info,metadata");
            cmd.arg("-C").arg(format!("metadata={}", short_key));
            cmd.arg("-C").arg(format!("extra-filename=-{}", short_key));
            cmd.arg("--out-dir").arg(self.out_dir);
        } else if is_bin {
            cmd.arg("--crate-type").arg("bin");
            cmd.arg("--emit=dep-info,link");
            cmd.arg("--out-dir").arg(self.out_dir);
        } else {
            // Standard library: rlib with metadata pipelining
            cmd.arg("--crate-type").arg("rlib");
            cmd.arg("--emit=dep-info,metadata,link");
            cmd.arg("-C").arg(format!("metadata={}", short_key));
            cmd.arg("-C").arg(format!("extra-filename=-{}", short_key));
            cmd.arg("--out-dir").arg(self.out_dir);
        }

        // Optimization and debug symbols
        if self.release {
            cmd.arg("-C").arg("opt-level=3");
        } else {
            cmd.arg("-C").arg("debuginfo=2");
            cmd.arg("-C").arg("embed-bitcode=no");
        }

        if !self.unit.is_workspace {
            cmd.arg("--cap-lints").arg("allow");
        } else {
            let pkg_dir = self.unit.manifest_path.parent().unwrap_or(Path::new("."));
            let ws_root = find_workspace_root(&self.unit.manifest_path);
            if pkg_dir.starts_with(&ws_root) {
                let ws_str = ws_root.to_string_lossy();
                let ws_clean = ws_str.trim_end_matches('/');
                cmd.arg(format!("--remap-path-prefix={}/=", ws_clean));
                cmd.arg(format!("--remap-path-prefix={}=", ws_clean));
            }
        }

        // Out-dir remappings must follow workspace remapping so that if OUT_DIR or staging
        // is inside the workspace (e.g. tests or MITTS_CACHE_DIR=./.cache), rustc's last-matching
        // rule prioritizes out_dir over the broader workspace prefix.
        let clean_crate = self.unit.target_name.replace('-', "_");
        let stable_out = format!("/mitts/out/{}", clean_crate);
        let stable_out_slash = format!("/mitts/out/{}/", clean_crate);

        let out_str = self.out_dir.to_string_lossy();
        let out_clean = out_str.trim_end_matches('/');
        cmd.arg(format!(
            "--remap-path-prefix={}/={}",
            out_clean, stable_out_slash
        ));
        cmd.arg(format!("--remap-path-prefix={}={}", out_clean, stable_out));

        if let Some(dir) = self.directives {
            if let Some(ref d_out) = dir.out_dir {
                let d_str = d_out.to_string_lossy();
                let d_clean = d_str.trim_end_matches('/');
                cmd.arg(format!(
                    "--remap-path-prefix={}/={}",
                    d_clean, stable_out_slash
                ));
                cmd.arg(format!("--remap-path-prefix={}={}", d_clean, stable_out));
            }
        }

        // Features
        for feat in self.features {
            cmd.arg("--cfg").arg(format!("feature=\"{}\"", feat));
        }

        // Build script directives (cfgs, check-cfgs, flags, link-libs, link-searches, link-args)
        if let Some(dir) = self.directives {
            for c in &dir.cfgs {
                cmd.arg("--cfg").arg(c);
            }
            for cc in &dir.check_cfgs {
                cmd.arg("--check-cfg").arg(cc);
            }
            for f in &dir.flags {
                for part in f.split_whitespace() {
                    cmd.arg(part);
                }
            }
            for l in &dir.link_libs {
                cmd.arg("-l").arg(l);
            }
            for s in &dir.link_searches {
                cmd.arg("-L").arg(s);
            }
            for a in &dir.link_args {
                cmd.arg("-C").arg(format!("link-arg={}", a));
            }
        }

        // Direct store reads via --extern
        for (dep_crate, dep_artifact_path) in self.extern_crates {
            let clean_dep = dep_crate.replace('-', "_");
            cmd.arg("--extern")
                .arg(format!("{}={}", clean_dep, dep_artifact_path.display()));
        }

        // Working directory and Cargo-compatible environment variables
        fs::create_dir_all(self.out_dir)?;
        let cwd = if self.unit.is_workspace {
            find_workspace_root(&self.unit.manifest_path)
        } else {
            self.unit
                .manifest_path
                .parent()
                .filter(|p| p.is_dir())
                .unwrap_or(self.out_dir)
                .to_path_buf()
        };
        cmd.current_dir(cwd);

        let unit_env = super::unit_env::compute_unit_rustc_env(
            self.unit,
            self.directives,
            self.jobserver_auth,
        );
        for (k, v) in &unit_env {
            cmd.env(k, v);
        }
        cmd.env("RUSTC", &compiler_bin);
        if let Some(toolchain_name) = compiler_bin
            .parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.file_name())
        {
            if compiler_bin.to_string_lossy().contains(".rustup/toolchains") {
                cmd.env("RUSTUP_TOOLCHAIN", toolchain_name);
            }
        }

        for arg in self.extra_compiler_args {
            cmd.arg(arg);
        }

        if !self.unit.is_workspace {
            cmd.arg("--cap-lints").arg("allow");
        }
        let is_pipelined_lib = self.unit.kind == "lib"
            && self.mode != BuildMode::Check
            && self.mode != BuildMode::Clippy;

        if is_pipelined_lib {
            cmd.arg("--error-format=json").arg("--json=artifacts");
        } else {
            cmd.arg("--color=never");
        }

        let command_line = format!("{:?}", cmd);

        let (success, exit_code, stdout, stderr, mut primary_output, mut metadata_output) =
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
                let exit_code = output.status.code().unwrap_or(1);
                let success = output.status.success();
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                (success, exit_code, stdout, stderr, None, None)
            };

        if success {
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
