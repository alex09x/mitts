/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::types::{BuildProgressEvent, CargoMessage, RustBuildOptions};
use crate::core::action::{Action, ActionKind, ActionResult, BuildPlan, ToolchainInfo};
use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Internal helper executing cargo build directly without caching.
pub(crate) fn run_cargo_build(
    manifest_path: &Path,
    target_dir: &Path,
    toolchain: &ToolchainInfo,
    opt: &RustBuildOptions,
) -> Result<(BuildPlan, ActionResult)> {
    let mut cmd = Command::new("cargo");
    cmd.arg("build");
    cmd.arg("--manifest-path").arg(manifest_path);
    cmd.arg("--target-dir").arg(target_dir);
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

    // Drain stderr concurrently to prevent pipe buffer deadlock
    let progress_cb = opt.progress.clone();
    let stderr_thread = std::thread::spawn(move || -> std::io::Result<String> {
        let mut recorded_stderr = String::new();
        let reader = BufReader::new(stderr);
        for line in reader.lines() {
            let line = line?;
            recorded_stderr.push_str(&line);
            recorded_stderr.push('\n');

            let trimmed = line.trim_start();
            if let Some(ref cb) = progress_cb {
                if let Some(crate_info) = trimmed.strip_prefix("Compiling ") {
                    let display = crate_info.split(" (").next().unwrap_or(crate_info);
                    cb.0(BuildProgressEvent::Compiling(display.trim().to_string()));
                } else if let Some(crate_info) = trimmed.strip_prefix("Checking ") {
                    let display = crate_info.split(" (").next().unwrap_or(crate_info);
                    cb.0(BuildProgressEvent::Compiling(format!("Checking {}", display.trim())));
                } else if let Some(crate_info) = trimmed.strip_prefix("Downloading ") {
                    let display = crate_info.split(" (").next().unwrap_or(crate_info);
                    cb.0(BuildProgressEvent::Compiling(format!("Downloading {}", display.trim())));
                } else if trimmed.starts_with("Finished ") || trimmed.starts_with("Fresh ") {
                    // Suppress build completion line; handled by Mitts stage summary
                } else if !trimmed.is_empty() {
                    cb.0(BuildProgressEvent::Diagnostic(line));
                }
            }
        }
        Ok(recorded_stderr)
    });

    let mut actions = Vec::new();
    let mut recorded_stdout = String::new();

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
                    let (kind, kind_str) = if target.kind.iter().any(|k| k == "bin") {
                        (ActionKind::CompileBin, "bin")
                    } else if target.kind.iter().any(|k| k == "lib" || k == "rlib") {
                        (ActionKind::CompileRlib, "lib")
                    } else if target.kind.iter().any(|k| k == "proc-macro") {
                        (ActionKind::Custom("proc-macro".to_string()), "proc-macro")
                    } else if target.kind.iter().any(|k| k == "custom-build") {
                        (ActionKind::BuildScript, "custom-build")
                    } else {
                        (ActionKind::Custom(target.kind.join(",")), "custom")
                    };

                    let outputs = filenames.into_iter().map(PathBuf::from).collect();
                    let inputs = vec![PathBuf::from(&target.src_path)];

                    actions.push(Action {
                        id: format!("{}::{}:{}", package_id, kind_str, target.name),
                        kind,
                        description: format!("Compile {} {} ({})", kind_str, target.name, package_id),
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
                    if let Some(ref cb) = opt.progress {
                        cb.0(BuildProgressEvent::CompilerMessage(r));
                    } else {
                        eprint!("{}", r);
                    }
                }
                _ => {}
            }
        }
    }

    let recorded_stderr = match stderr_thread.join() {
        Ok(res) => res?,
        Err(_) => bail!("Failed to join stderr reader thread"),
    };

    let status = child.wait().context("Failed to wait on cargo child process")?;
    let exit_code = {
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            if let Some(c) = status.code() {
                c
            } else if let Some(sig) = status.signal() {
                128 + sig
            } else {
                1
            }
        }
        #[cfg(not(unix))]
        {
            status.code().unwrap_or(1)
        }
    };
    let success = status.success();

    let plan = BuildPlan {
        actions,
        target_directory: target_dir.to_path_buf(),
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
