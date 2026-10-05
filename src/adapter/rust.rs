/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::core::action::{Action, ActionKind, ActionResult, BuildPlan, ToolchainInfo};
use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Debug, thiserror::Error)]
pub enum RustAdapterError {
    #[error("Manifest not found at {0}")]
    ManifestNotFound(PathBuf),
    #[error("Unsupported build option: {0}")]
    UnsupportedOption(String),
    #[error("Cargo execution failed with exit code {0}")]
    CargoFailed(i32),
}

#[derive(Debug, Clone, Default)]
pub struct RustBuildOptions {
    pub manifest_path: Option<PathBuf>,
    pub package: Option<String>,
    pub release: bool,
    pub target_dir: Option<PathBuf>,
    pub features: Vec<String>,
    pub no_default_features: bool,
    pub all_features: bool,
    pub extra_args: Vec<String>,
}

/// JSON message shapes emitted by Cargo with `--message-format=json`.
#[derive(Debug, Deserialize)]
#[serde(tag = "reason")]
#[allow(dead_code)]
enum CargoMessage {
    #[serde(rename = "compiler-artifact")]
    CompilerArtifact {
        package_id: String,
        target: CargoTarget,
        profile: CargoProfile,
        features: Vec<String>,
        filenames: Vec<String>,
        executable: Option<String>,
        fresh: bool,
    },
    #[serde(rename = "build-script-executed")]
    BuildScriptExecuted {
        package_id: String,
        linked_libs: Vec<String>,
        linked_paths: Vec<String>,
    },
    #[serde(rename = "compiler-message")]
    CompilerMessage {
        rendered: Option<String>,
    },
    #[serde(rename = "build-finished")]
    BuildFinished {
        success: bool,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct CargoTarget {
    name: String,
    kind: Vec<String>,
    src_path: String,
    edition: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct CargoProfile {
    opt_level: String,
    debuginfo: Option<serde_json::Value>,
    test: bool,
}

pub struct RustAdapter;

impl RustAdapter {
    /// Detects current installed rustc toolchain info.
    pub fn detect_toolchain() -> Result<ToolchainInfo> {
        let output = Command::new("rustc")
            .arg("--version")
            .output()
            .context("Failed to invoke rustc --version")?;
        let version_str = String::from_utf8_lossy(&output.stdout).trim().to_string();

        let target_output = Command::new("rustc")
            .args(["-vV"])
            .output()
            .ok();
        let mut target_triple = None;
        if let Some(t_out) = target_output {
            let text = String::from_utf8_lossy(&t_out.stdout);
            for line in text.lines() {
                if let Some(rest) = line.strip_prefix("host: ") {
                    target_triple = Some(rest.trim().to_string());
                }
            }
        }

        Ok(ToolchainInfo {
            name: "rustc".to_string(),
            version: version_str,
            target_triple,
        })
    }

    /// Resolves the canonical manifest path for the target build.
    pub fn resolve_manifest(opt: &RustBuildOptions) -> Result<PathBuf> {
        if let Some(ref p) = opt.manifest_path {
            let path = if p.is_absolute() {
                p.clone()
            } else {
                std::env::current_dir()?.join(p)
            };
            if !path.exists() {
                return Err(RustAdapterError::ManifestNotFound(path).into());
            }
            Ok(path)
        } else {
            let default_path = std::env::current_dir()?.join("Cargo.toml");
            if !default_path.exists() {
                return Err(RustAdapterError::ManifestNotFound(default_path).into());
            }
            Ok(default_path)
        }
    }

    /// Resolves the dedicated isolated Mitts target directory for the workspace.
    pub fn resolve_target_dir(manifest_path: &Path, opt: &RustBuildOptions) -> Result<PathBuf> {
        if let Some(ref td) = opt.target_dir {
            Ok(td.clone())
        } else {
            let project_dir = manifest_path
                .parent()
                .context("Manifest path has no parent directory")?;
            Ok(project_dir.join(".mitts").join("target"))
        }
    }

    /// Executes the uncached build according to the M1 contract.
    pub fn build(opt: &RustBuildOptions) -> Result<(BuildPlan, ActionResult)> {
        let manifest_path = Self::resolve_manifest(opt)?;
        let target_dir = Self::resolve_target_dir(&manifest_path, opt)?;
        let toolchain = Self::detect_toolchain()?;

        // Validate unsupported options explicitly
        for arg in &opt.extra_args {
            if arg.starts_with("--unsupported") {
                return Err(RustAdapterError::UnsupportedOption(arg.clone()).into());
            }
        }

        let mut cmd = Command::new("cargo");
        cmd.arg("build");
        cmd.arg("--manifest-path").arg(&manifest_path);
        cmd.arg("--target-dir").arg(&target_dir);
        cmd.arg("--message-format=json-diagnostic-rendered-ansi");

        if opt.release {
            cmd.arg("--release");
        }
        if let Some(ref pkg) = opt.package {
            cmd.arg("--package").arg(pkg);
        }
        if !opt.features.is_empty() {
            cmd.arg("--features").arg(opt.features.join(","));
        }
        if opt.no_default_features {
            cmd.arg("--no-default-features");
        }
        if opt.all_features {
            cmd.arg("--all-features");
        }
        for extra in &opt.extra_args {
            cmd.arg(extra);
        }

        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        let mut child = cmd.spawn().context("Failed to spawn cargo build")?;
        let stdout = child.stdout.take().context("Failed to open stdout")?;
        let stderr = child.stderr.take().context("Failed to open stderr")?;

        let mut actions = Vec::new();
        let mut recorded_stdout = String::new();
        let mut recorded_stderr = String::new();

        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            let line = line?;
            recorded_stdout.push_str(&line);
            recorded_stdout.push('\n');

            if let Ok(msg) = serde_json::from_str::<CargoMessage>(&line) {
                match msg {
                    CargoMessage::CompilerArtifact {
                        package_id,
                        target,
                        filenames,
                        ..
                    } => {
                        let kind = if target.kind.iter().any(|k| k == "bin") {
                            ActionKind::CompileBin
                        } else if target.kind.iter().any(|k| k == "lib" || k == "rlib") {
                            ActionKind::CompileRlib
                        } else {
                            ActionKind::Custom(target.kind.join(","))
                        };

                        let outputs = filenames.into_iter().map(PathBuf::from).collect();
                        let inputs = vec![PathBuf::from(&target.src_path)];

                        actions.push(Action {
                            id: format!("{}-{}", target.name, package_id),
                            kind,
                            description: format!("Compile {} ({})", target.name, package_id),
                            toolchain: toolchain.clone(),
                            inputs,
                            outputs,
                            args: Vec::new(),
                            env: HashMap::new(),
                            dependencies: Vec::new(),
                        });
                    }
                    CargoMessage::BuildScriptExecuted {
                        package_id, ..
                    } => {
                        actions.push(Action {
                            id: format!("build-script-{}", package_id),
                            kind: ActionKind::BuildScript,
                            description: format!("Run build script for {}", package_id),
                            toolchain: toolchain.clone(),
                            inputs: Vec::new(),
                            outputs: Vec::new(),
                            args: Vec::new(),
                            env: HashMap::new(),
                            dependencies: Vec::new(),
                        });
                    }
                    CargoMessage::CompilerMessage { rendered: Some(r) } => {
                        eprint!("{}", r);
                    }
                    _ => {}
                }
            }
        }

        let stderr_reader = BufReader::new(stderr);
        for line in stderr_reader.lines() {
            let line = line?;
            recorded_stderr.push_str(&line);
            recorded_stderr.push('\n');
            eprintln!("{}", line);
        }

        let status = child.wait().context("Failed to wait on cargo child process")?;
        let exit_code = status.code().unwrap_or(1);
        let success = status.success();

        let plan = BuildPlan {
            actions,
            target_directory: target_dir,
        };

        let result = ActionResult {
            action_id: "build-all".to_string(),
            exit_code,
            success,
            stdout: recorded_stdout,
            stderr: recorded_stderr,
            cached: false,
        };

        Ok((plan, result))
    }
}
