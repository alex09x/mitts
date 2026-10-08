/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::manifest::CargoToml;
use super::types::PlannedCompilationUnit;
use anyhow::{Context, Result};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Classification of dependency requirement between two compilation units.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepRequirement {
    /// Consumer only needs metadata (.rmeta) from the dependency to begin compilation.
    MetadataOnly,
    /// Consumer needs full machine code (.rlib, .so, etc.) from the dependency.
    FullArtifact,
}

/// Returns true if the package described by manifest_path has a configured build script that exists.
pub fn package_has_build_script(manifest_path: &Path) -> bool {
    let pkg_dir = manifest_path.parent().unwrap_or(Path::new("."));
    if let Ok(pkg_toml) = CargoToml::from_file(manifest_path) {
        match pkg_toml.package.as_ref().and_then(|p| p.build.as_ref()) {
            Some(super::manifest::TomlBuild::Disabled(false)) => false,
            Some(super::manifest::TomlBuild::Path(ref rel_p)) => pkg_dir.join(rel_p).is_file(),
            _ => pkg_dir.join("build.rs").is_file() || pkg_dir.join("build").join("build.rs").is_file(),
        }
    } else {
        pkg_dir.join("build.rs").is_file() || pkg_dir.join("build").join("build.rs").is_file()
    }
}

/// Classifies whether a consumer unit requires only metadata or the full artifact from a dependency.
pub fn classify_dep_requirement(
    consumer: &PlannedCompilationUnit,
    dependency: &PlannedCompilationUnit,
) -> DepRequirement {
    // Proc macros are compiler plugins: rustc must load the compiled shared library (.so/.dylib)
    if dependency.kind == "proc-macro" {
        return DepRequirement::FullArtifact;
    }
    // Linking targets need native code from all their dependencies
    let is_consumer_linking = matches!(
        consumer.kind.as_str(),
        "bin" | "test" | "proc-macro" | "dylib" | "cdylib" | "example" | "bench"
    );
    if is_consumer_linking {
        return DepRequirement::FullArtifact;
    }
    // If consumer package has a build script, build dependencies must be FullArtifact (.rlib)
    if package_has_build_script(&consumer.manifest_path) {
        if let Ok(toml) = CargoToml::from_file(&consumer.manifest_path) {
            let dep_pkg = dependency.package_name.replace('-', "_");
            let is_build_dep = toml
                .build_dependencies
                .keys()
                .any(|k| k.replace('-', "_") == dep_pkg)
                || toml.target.values().any(|t| {
                    t.build_dependencies
                        .keys()
                        .any(|k| k.replace('-', "_") == dep_pkg)
                });
            if is_build_dep {
                return DepRequirement::FullArtifact;
            }
        }
    }
    // Normal library depending on another library only requires metadata for type checking
    DepRequirement::MetadataOnly
}

/// Parses an artifact JSON line from rustc stderr.
pub fn parse_rustc_artifact_line(line: &str) -> Option<(PathBuf, String)> {
    if !line.contains("\"$message_type\":\"artifact\"") {
        return None;
    }
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
        if v.get("$message_type").and_then(|s| s.as_str()) == Some("artifact") {
            let artifact = v.get("artifact").and_then(|s| s.as_str())?;
            let emit = v
                .get("emit")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string();
            return Some((PathBuf::from(artifact), emit));
        }
    }
    None
}

/// Parses a rendered diagnostic message from rustc JSON stderr.
pub fn parse_rustc_rendered_diagnostic(line: &str) -> Option<String> {
    if !line.contains("\"$message_type\":\"diagnostic\"") {
        return None;
    }
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
        if let Some(rendered) = v.get("rendered").and_then(|s| s.as_str()) {
            return Some(rendered.to_string());
        }
    }
    None
}

/// Result from executing a pipelined rustc child process.
pub struct PipelinedRustcResult {
    pub success: bool,
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub primary_output: Option<PathBuf>,
    pub metadata_output: Option<PathBuf>,
}

/// Executes a `rustc` process with pipelined metadata artifact notification.
pub fn run_pipelined_rustc<F>(mut cmd: Command, on_metadata: F) -> Result<PipelinedRustcResult>
where
    F: FnOnce(&Path),
{
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = cmd.spawn().context("Failed to spawn rustc child process")?;

    let stdout_pipe = child.stdout.take().context("Failed to capture stdout")?;
    let stderr_pipe = child.stderr.take().context("Failed to capture stderr")?;

    let stdout_handle = std::thread::spawn(move || {
        let mut out = String::new();
        let reader = BufReader::new(stdout_pipe);
        for line in reader.lines().map_while(Result::ok) {
            out.push_str(&line);
            out.push('\n');
        }
        out
    });

    let mut on_meta_opt = Some(on_metadata);
    let mut collected_stderr = String::new();
    let mut primary_output = None;
    let mut metadata_output = None;

    let stderr_reader = BufReader::new(stderr_pipe);
    for line in stderr_reader.lines().map_while(Result::ok) {
        if let Some((artifact_path, emit)) = parse_rustc_artifact_line(&line) {
            if emit == "metadata" {
                metadata_output = Some(artifact_path.clone());
                if let Some(cb) = on_meta_opt.take() {
                    cb(&artifact_path);
                }
            } else if emit == "link" {
                primary_output = Some(artifact_path);
            }
        } else if let Some(rendered) = parse_rustc_rendered_diagnostic(&line) {
            collected_stderr.push_str(&rendered);
        } else {
            collected_stderr.push_str(&line);
            collected_stderr.push('\n');
        }
    }

    let status = child
        .wait()
        .context("Failed to wait on rustc child process")?;
    let stdout_str = stdout_handle
        .join()
        .unwrap_or_else(|_| "<stdout thread panic>".to_string());

    let exit_code = status
        .code()
        .unwrap_or(if status.success() { 0 } else { 1 });
    let success = status.success();

    Ok(PipelinedRustcResult {
        success,
        exit_code,
        stdout: stdout_str,
        stderr: collected_stderr,
        primary_output,
        metadata_output,
    })
}
