/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::types::PlannedCompilationUnit;
use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Results from invoking `rustc`.
#[derive(Debug, Clone)]
pub struct RustcOutput {
    pub success: bool,
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub primary_output: Option<PathBuf>,
    pub metadata_output: Option<PathBuf>,
}

/// Output captured from running a `build.rs` build script.
#[derive(Debug, Clone, Default)]
pub struct BuildScriptDirectives {
    pub cfgs: Vec<String>,
    pub link_libs: Vec<String>,
    pub link_searches: Vec<String>,
    pub envs: HashMap<String, String>,
    pub out_dir: Option<PathBuf>,
}

/// Direct `rustc` command invocation builder.
pub struct RustcInvocation<'a> {
    pub unit: &'a PlannedCompilationUnit,
    pub action_key: &'a str,
    pub out_dir: &'a Path,
    pub dest_binary: Option<&'a Path>,
    pub extern_crates: &'a [(String, PathBuf)],
    pub release: bool,
    pub features: &'a [String],
    pub directives: Option<&'a BuildScriptDirectives>,
}

impl<'a> RustcInvocation<'a> {
    /// Executes the direct `rustc` compilation.
    pub fn execute(self) -> Result<RustcOutput> {
        let clean_crate = self.unit.target_name.replace('-', "_");
        let edition = self.unit.edition.as_deref().unwrap_or("2021");
        let short_key = if self.action_key.len() > 16 {
            &self.action_key[..16]
        } else {
            self.action_key
        };

        let mut cmd = Command::new("rustc");
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
                .join(if self.unit.kind == "bin" { "main.rs" } else { "lib.rs" });
            if store_src.is_file() {
                store_src
            } else {
                self.unit.src_path.clone()
            }
        };
        cmd.arg(&actual_src);
        cmd.arg("--edition").arg(edition);

        let is_bin = self.unit.kind == "bin";
        let is_proc_macro = self.unit.kind == "proc-macro";

        if is_bin {
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
        } else if is_proc_macro {
            cmd.arg("--crate-type").arg("proc-macro");
            cmd.arg("-C").arg("prefer-dynamic");
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
        }

        // Features
        for feat in self.features {
            cmd.arg("--cfg").arg(format!("feature=\"{}\"", feat));
        }

        // Build script directives (cfgs, link-libs, etc.)
        if let Some(dir) = self.directives {
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
            cmd.arg("--extern").arg(format!("{}={}", clean_dep, dep_artifact_path.display()));
            if let Some(parent) = dep_artifact_path.parent() {
                cmd.arg("-L").arg(format!("dependency={}", parent.display()));
            }
        }

        // Cargo-compatible environment variables
        let pkg_dir = self.unit.manifest_path.parent().unwrap_or(Path::new("."));
        cmd.env("CARGO_MANIFEST_DIR", pkg_dir);
        cmd.env("CARGO_PKG_NAME", &self.unit.package_name);
        cmd.env("CARGO_PKG_VERSION", &self.unit.version);
        cmd.env("CARGO_CRATE_NAME", &self.unit.target_name);

        let output = cmd.output().with_context(|| {
            format!("Failed to execute rustc for unit '{}'", self.unit.unit_id())
        })?;

        let exit_code = output.status.code().unwrap_or(1);
        let success = output.status.success();
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        let mut primary_output = None;
        let mut metadata_output = None;

        if success {
            if is_bin {
                if let Some(dest) = self.dest_binary {
                    primary_output = Some(dest.to_path_buf());
                } else {
                    let bin_path = self.out_dir.join(&clean_crate);
                    if bin_path.is_file() {
                        primary_output = Some(bin_path);
                    }
                }
            } else {
                // Find generated .rlib / .dylib and .rmeta in out_dir
                if let Ok(entries) = fs::read_dir(self.out_dir) {
                    for entry in entries.flatten() {
                        let p = entry.path();
                        if let Some(ext) = p.extension() {
                            if ext == "rlib" || ext == "dylib" || ext == "so" {
                                primary_output = Some(p.clone());
                            } else if ext == "rmeta" {
                                metadata_output = Some(p);
                            }
                        }
                    }
                }
            }
        }

        Ok(RustcOutput {
            success,
            exit_code,
            stdout,
            stderr,
            primary_output,
            metadata_output,
        })
    }
}

/// Executes a crate's `build.rs` build script in an isolated sandbox.
pub fn execute_build_script(
    unit: &PlannedCompilationUnit,
    staging_dir: &Path,
) -> Result<BuildScriptDirectives> {
    let pkg_dir = unit.manifest_path.parent().unwrap_or(Path::new("."));
    let build_rs = pkg_dir.join("build.rs");
    if !build_rs.is_file() {
        return Ok(BuildScriptDirectives::default());
    }

    let bin_path = staging_dir.join("build_script_build");
    let out_dir = staging_dir.join("out");
    fs::create_dir_all(&out_dir)?;

    // Compile build.rs
    let compile_status = Command::new("rustc")
        .arg("--crate-name")
        .arg("build_script_build")
        .arg(&build_rs)
        .arg("--crate-type")
        .arg("bin")
        .arg("--edition")
        .arg(unit.edition.as_deref().unwrap_or("2021"))
        .arg("-o")
        .arg(&bin_path)
        .output()
        .context("Failed to compile build.rs")?;

    if !compile_status.status.success() {
        let err = String::from_utf8_lossy(&compile_status.stderr);
        bail!("Failed to compile build.rs for '{}': {}", unit.package_name, err);
    }

    // Run build.rs
    let run_status = Command::new(&bin_path)
        .env("CARGO_MANIFEST_DIR", pkg_dir)
        .env("CARGO_PKG_NAME", &unit.package_name)
        .env("CARGO_PKG_VERSION", &unit.version)
        .env("OUT_DIR", &out_dir)
        .env("TARGET", std::env::var("TARGET").unwrap_or_else(|_| "host".to_string()))
        .env("HOST", std::env::var("HOST").unwrap_or_else(|_| "host".to_string()))
        .env("PROFILE", "debug")
        .output()
        .context("Failed to execute build.rs")?;

    if !run_status.status.success() {
        let err = String::from_utf8_lossy(&run_status.stderr);
        bail!("build.rs failed for '{}': {}", unit.package_name, err);
    }

    let mut directives = BuildScriptDirectives {
        out_dir: Some(out_dir),
        ..Default::default()
    };

    let stdout_str = String::from_utf8_lossy(&run_status.stdout);
    for line in stdout_str.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("cargo:rustc-cfg=") {
            directives.cfgs.push(rest.to_string());
        } else if let Some(rest) = trimmed.strip_prefix("cargo:rustc-link-lib=") {
            directives.link_libs.push(rest.to_string());
        } else if let Some(rest) = trimmed.strip_prefix("cargo:rustc-link-search=") {
            let val = rest.strip_prefix("native=").unwrap_or(rest);
            directives.link_searches.push(val.to_string());
        } else if let Some(rest) = trimmed.strip_prefix("cargo:rustc-env=") {
            if let Some((k, v)) = rest.split_once('=') {
                directives.envs.insert(k.to_string(), v.to_string());
            }
        }
    }

    Ok(directives)
}
