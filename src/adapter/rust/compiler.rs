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

/// Results from invoking `rustc` or `clippy-driver`.
#[derive(Debug, Clone)]
pub struct RustcOutput {
    pub success: bool,
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub command_line: String,
    pub primary_output: Option<PathBuf>,
    pub metadata_output: Option<PathBuf>,
}

pub fn resolve_rustc() -> PathBuf {
    if let Ok(p) = std::env::var("RUSTC") {
        return PathBuf::from(p);
    }
    if let Ok(paths) = std::env::var("PATH") {
        for dir in std::env::split_paths(&paths) {
            let p = dir.join("rustc");
            if p.is_file() {
                return p;
            }
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        let p = PathBuf::from(home).join(".cargo").join("bin").join("rustc");
        if p.is_file() {
            return p;
        }
    }
    PathBuf::from("rustc")
}

pub fn resolve_clippy_driver() -> PathBuf {
    if let Ok(p) = std::env::var("CLIPPY_DRIVER") {
        return PathBuf::from(p);
    }
    if let Ok(home) = std::env::var("HOME") {
        let p = PathBuf::from(home)
            .join(".cargo")
            .join("bin")
            .join("clippy-driver");
        if p.is_file() {
            return p;
        }
    }
    PathBuf::from("clippy-driver")
}

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
            if let Some(dest) = self.dest_binary {
                if let Some(parent) = dest.parent() {
                    fs::create_dir_all(parent)?;
                }
                cmd.arg("-o").arg(dest);
            } else {
                cmd.arg("--out-dir").arg(self.out_dir);
            }
        } else if is_proc_macro {
            cmd.arg("--crate-type").arg("proc-macro");
            cmd.arg("--extern").arg("proc_macro");
            cmd.arg("-C").arg("prefer-dynamic");
            cmd.arg("--emit=dep-info,link");
            cmd.arg("-C").arg(format!("metadata={}", short_key));
            cmd.arg("-C").arg(format!("extra-filename=-{}", short_key));
            cmd.arg("--out-dir").arg(self.out_dir);
        } else if (self.mode == BuildMode::Check || self.mode == BuildMode::Clippy)
            && (self.unit.is_workspace
                || self
                    .extern_crates
                    .iter()
                    .any(|(_, p)| p.extension().and_then(|e| e.to_str()) == Some("rmeta")))
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
            if let Some(dest) = self.dest_binary {
                if let Some(parent) = dest.parent() {
                    fs::create_dir_all(parent)?;
                }
                cmd.arg("-o").arg(dest);
            } else {
                cmd.arg("--out-dir").arg(self.out_dir);
            }
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
        }

        // Features
        for feat in self.features {
            cmd.arg("--cfg").arg(format!("feature=\"{}\"", feat));
            cmd.env(
                format!("CARGO_FEATURE_{}", feat.to_uppercase().replace('-', "_")),
                "1",
            );
        }

        // Build script directives (cfgs, link-libs, OUT_DIR, etc.)
        if let Some(dir) = self.directives {
            if let Some(ref out) = dir.out_dir {
                cmd.env("OUT_DIR", out);
            }
            for cfg_item in &dir.cfgs {
                cmd.arg("--cfg").arg(cfg_item);
            }
            for lib in &dir.link_libs {
                cmd.arg("-l").arg(lib);
            }
            for search in &dir.link_searches {
                cmd.arg("-L").arg(search);
            }
            for (k, v) in &dir.envs {
                cmd.env(k, v);
            }
        }

        // Direct store reads via --extern
        for (dep_crate, dep_artifact_path) in self.extern_crates {
            let clean_dep = dep_crate.replace('-', "_");
            cmd.arg("--extern")
                .arg(format!("{}={}", clean_dep, dep_artifact_path.display()));
            if let Some(parent) = dep_artifact_path.parent() {
                cmd.arg("-L")
                    .arg(format!("dependency={}", parent.display()));
            }
        }

        let (major, minor, patch, pre) = if let Ok(s) = semver::Version::parse(&self.unit.version) {
            (
                s.major.to_string(),
                s.minor.to_string(),
                s.patch.to_string(),
                s.pre.as_str().to_string(),
            )
        } else {
            let mut v = self.unit.version.split('.');
            let (maj, min) = (v.next().unwrap_or("0"), v.next().unwrap_or("0"));
            let p_pre = v.next().unwrap_or("0");
            let (pat, p) = p_pre.split_once('-').unwrap_or((p_pre, ""));
            (
                maj.to_string(),
                min.to_string(),
                pat.to_string(),
                p.to_string(),
            )
        };

        // Cargo-compatible environment variables and working directory
        let pkg_dir = self.unit.manifest_path.parent().unwrap_or(Path::new("."));
        fs::create_dir_all(self.out_dir)?;
        cmd.current_dir(self.out_dir);
        if let Ok(tc) = std::env::var("RUSTUP_TOOLCHAIN") {
            cmd.env("RUSTUP_TOOLCHAIN", tc);
        }
        if let Some(auth) = self.jobserver_auth {
            cmd.env("CARGO_MAKEFLAGS", format!("--jobserver-auth={}", auth));
            cmd.env("MAKEFLAGS", format!("--jobserver-auth={}", auth));
        }
        cmd.env("CARGO_MANIFEST_DIR", pkg_dir);
        let cargo_bin = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
        cmd.env("CARGO", &cargo_bin);
        cmd.env("CARGO_PKG_NAME", &self.unit.package_name);
        cmd.env("CARGO_PKG_VERSION", &self.unit.version);
        cmd.env("CARGO_PKG_VERSION_MAJOR", &major);
        cmd.env("CARGO_PKG_VERSION_MINOR", &minor);
        cmd.env("CARGO_PKG_VERSION_PATCH", &patch);
        cmd.env("CARGO_PKG_VERSION_PRE", &pre);
        cmd.env("CARGO_CRATE_NAME", &self.unit.target_name);
        if is_bin {
            cmd.env("CARGO_BIN_NAME", &self.unit.target_name);
        }

        if let Ok(pkg_toml) = super::manifest::CargoToml::from_file(&self.unit.manifest_path) {
            if let Some(ref p) = pkg_toml.package {
                cmd.env(
                    "CARGO_PKG_DESCRIPTION",
                    p.description.as_deref().unwrap_or(""),
                );
                cmd.env("CARGO_PKG_HOMEPAGE", p.homepage.as_deref().unwrap_or(""));
                cmd.env(
                    "CARGO_PKG_REPOSITORY",
                    p.repository.as_deref().unwrap_or(""),
                );
                cmd.env("CARGO_PKG_LICENSE", p.license.as_deref().unwrap_or(""));
                cmd.env("CARGO_PKG_README", p.readme.as_deref().unwrap_or(""));
                let authors = p.authors.as_deref().unwrap_or(&[]).join(":");
                cmd.env("CARGO_PKG_AUTHORS", authors);
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
                if let Some(dest) = self.dest_binary {
                    primary_output = Some(dest.to_path_buf());
                } else {
                    let bin_path = self.out_dir.join(&clean_crate);
                    if bin_path.is_file() {
                        primary_output = Some(bin_path);
                    }
                }
            }

            // Find generated .rlib / .dylib and .rmeta in out_dir if not captured by streaming
            if primary_output.is_none() || metadata_output.is_none() {
                if let Ok(entries) = fs::read_dir(self.out_dir) {
                    for entry in entries.flatten() {
                        let p = entry.path();
                        if let Some(ext) = p.extension() {
                            if (ext == "rlib" || ext == "dylib" || ext == "so")
                                && primary_output.is_none()
                            {
                                primary_output = Some(p);
                            } else if ext == "rmeta" && metadata_output.is_none() {
                                metadata_output = Some(p);
                            }
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
