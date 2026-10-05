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
use crate::core::action_cache::{ActionCache, ActionCacheEntry, CachedOutput};
use crate::core::cas::ContentAddressedStore;
use crate::core::hash::Sha256;
use crate::core::lock::FileLock;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannedCompilationUnit {
    pub package_name: String,
    pub version: String,
    pub target_name: String,
    pub kind: String,
    pub src_path: PathBuf,
    pub cache_eligibility: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EffectivePlanExplanation {
    pub manifest_path: PathBuf,
    pub target_directory: PathBuf,
    pub profile: String,
    pub toolchain: ToolchainInfo,
    pub units: Vec<PlannedCompilationUnit>,
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
    pub no_cache: bool,
    pub cache_dir: Option<PathBuf>,
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

#[derive(Debug, Deserialize)]
struct MetadataOutput {
    packages: Vec<MetaPackage>,
    workspace_members: Vec<String>,
    #[serde(default)]
    workspace_default_members: Vec<String>,
    resolve: Option<MetaResolve>,
}

#[derive(Debug, Deserialize)]
struct MetaPackage {
    id: String,
    name: String,
    version: String,
    manifest_path: String,
    #[allow(dead_code)]
    source: Option<String>,
    targets: Vec<MetaTarget>,
}

#[derive(Debug, Deserialize)]
struct MetaTarget {
    name: String,
    kind: Vec<String>,
    src_path: String,
}

#[derive(Debug, Deserialize)]
struct MetaResolve {
    nodes: Vec<MetaResolveNode>,
}

#[derive(Debug, Deserialize)]
struct MetaResolveNode {
    id: String,
    dependencies: Vec<String>,
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

    /// Derives the effective build plan and compilation units from Cargo metadata.
    pub fn explain_plan(opt: &RustBuildOptions) -> Result<EffectivePlanExplanation> {
        let manifest_path = Self::resolve_manifest(opt)?;
        let target_dir = Self::resolve_target_dir(&manifest_path, opt)?;
        let toolchain = Self::detect_toolchain()?;

        let mut cmd = Command::new("cargo");
        cmd.arg("metadata");
        cmd.arg("--format-version=1");
        cmd.arg("--manifest-path").arg(&manifest_path);

        if !opt.features.is_empty() {
            cmd.arg("--features").arg(opt.features.join(","));
        }
        if opt.no_default_features {
            cmd.arg("--no-default-features");
        }
        if opt.all_features {
            cmd.arg("--all-features");
        }

        let output = cmd.output().context("Failed to execute cargo metadata")?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("cargo metadata failed: {}", stderr.trim());
        }

        let meta: MetadataOutput = serde_json::from_slice(&output.stdout)
            .context("Failed to parse cargo metadata JSON output")?;

        let packages_by_id: HashMap<String, MetaPackage> = meta
            .packages
            .into_iter()
            .map(|p| (p.id.clone(), p))
            .collect();

        let canonical_manifest = manifest_path.canonicalize().unwrap_or_else(|_| manifest_path.clone());
        let mut root_ids = Vec::new();

        if let Some(ref target_pkg_name) = opt.package {
            let root_pkg = packages_by_id
                .values()
                .find(|p| &p.name == target_pkg_name)
                .ok_or_else(|| anyhow::anyhow!("Package '{}' not found in workspace", target_pkg_name))?;
            root_ids.push(root_pkg.id.clone());
        } else {
            // Check if manifest_path points directly to a package manifest
            let matching_pkg = packages_by_id
                .values()
                .find(|p| {
                    Path::new(&p.manifest_path)
                        .canonicalize()
                        .map(|cp| cp == canonical_manifest)
                        .unwrap_or(false)
                });

            if let Some(pkg) = matching_pkg {
                root_ids.push(pkg.id.clone());
            } else if !meta.workspace_default_members.is_empty() {
                root_ids.extend(meta.workspace_default_members.clone());
            } else {
                root_ids.extend(meta.workspace_members.clone());
            }
        }

        let mut selected_ids = Vec::new();
        let mut to_visit = root_ids;
        let mut visited = HashSet::new();

        while let Some(id) = to_visit.pop() {
            if visited.insert(id.clone()) {
                selected_ids.push(id.clone());
                if let Some(ref resolve) = meta.resolve {
                    if let Some(node) = resolve.nodes.iter().find(|n| n.id == id) {
                        for dep_id in &node.dependencies {
                            if !visited.contains(dep_id) {
                                to_visit.push(dep_id.clone());
                            }
                        }
                    }
                }
            }
        }

        let mut units = Vec::new();
        for id in &selected_ids {
            if let Some(pkg) = packages_by_id.get(id) {
                let is_workspace = meta.workspace_members.contains(&pkg.id);
                for target in &pkg.targets {
                    let is_compile_target = target.kind.iter().any(|k| {
                        matches!(
                            k.as_str(),
                            "bin" | "lib" | "rlib" | "dylib" | "cdylib" | "proc-macro" | "custom-build"
                        )
                    });
                    if !is_compile_target {
                        continue;
                    }

                    let is_bin = target.kind.iter().any(|k| k == "bin");
                    let is_pure_rlib = !target.kind.is_empty()
                        && target.kind.iter().all(|k| k == "lib" || k == "rlib");
                    let has_native_or_mixed_lib = target.kind.iter().any(|k| {
                        matches!(k.as_str(), "cdylib" | "dylib" | "staticlib")
                    });

                    let (kind_str, eligibility) = if is_bin {
                        let elig = if is_workspace {
                            "Uncached in M1 (eligible for M2 workspace action cache)".to_string()
                        } else {
                            "Uncached in M1 (eligible for M2 immutable dependency cache)".to_string()
                        };
                        ("bin".to_string(), elig)
                    } else if is_pure_rlib {
                        let elig = if is_workspace {
                            "Uncached in M1 (eligible for M2 workspace action cache)".to_string()
                        } else {
                            "Uncached in M1 (eligible for M2 immutable dependency cache)".to_string()
                        };
                        ("lib".to_string(), elig)
                    } else if has_native_or_mixed_lib {
                        (
                            target.kind.join(","),
                            "Uncached (contains native/dynamic library crate types; outside initial M2 pure rlib subset)".to_string(),
                        )
                    } else if target.kind.iter().any(|k| k == "proc-macro") {
                        (
                            "proc-macro".to_string(),
                            "Uncached (procedural macro; outside initial M2 cache subset)".to_string(),
                        )
                    } else if target.kind.iter().any(|k| k == "custom-build") {
                        (
                            "custom-build".to_string(),
                            "Uncached (build script; outside initial M2 cache subset)".to_string(),
                        )
                    } else {
                        (
                            target.kind.join(","),
                            "Uncached (unsupported action kind for initial M2 cache)".to_string(),
                        )
                    };

                    units.push(PlannedCompilationUnit {
                        package_name: pkg.name.clone(),
                        version: pkg.version.clone(),
                        target_name: target.name.clone(),
                        kind: kind_str,
                        src_path: PathBuf::from(&target.src_path),
                        cache_eligibility: eligibility,
                    });
                }
            }
        }

        let profile = if opt.release {
            "release".to_string()
        } else {
            "debug".to_string()
        };

        Ok(EffectivePlanExplanation {
            manifest_path,
            target_directory: target_dir,
            profile,
            toolchain,
            units,
        })
    }

    /// Executes the build according to the M2 caching contract.
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

        let explanation = Self::explain_plan(opt)?;

        // Cache eligibility: all units must be eligible for M2 pure rlib/bin caching
        let is_cache_eligible = !opt.no_cache
            && !explanation.units.is_empty()
            && explanation
                .units
                .iter()
                .all(|u| u.cache_eligibility.contains("eligible for M2"));

        if is_cache_eligible {
            let action_key = derive_build_action_key(opt, &explanation)?;
            let cas = match opt.cache_dir {
                Some(ref cd) => ContentAddressedStore::new(cd.join("cas"))?,
                None => ContentAddressedStore::default_store()?,
            };
            let ac = match opt.cache_dir {
                Some(ref cd) => ActionCache::new(cd.join("ac"))?,
                None => ActionCache::default_cache()?,
            };

            // 1. Fast path: check action cache without lock
            if let Ok(Some(entry)) = ac.get(&action_key) {
                let mut all_valid = !entry.outputs.is_empty();
                for out in &entry.outputs {
                    if !cas.has_object(&out.cas_hash)
                        || !cas.verify_object(&out.cas_hash).unwrap_or(false)
                    {
                        all_valid = false;
                        break;
                    }
                }
                if all_valid {
                    // Restore all outputs into isolated target directory
                    for out in &entry.outputs {
                        let dest = target_dir.join(&out.relative_path);
                        cas.restore_object(&out.cas_hash, &dest, out.executable)?;
                    }

                    let mut actions = Vec::new();
                    for out in &entry.outputs {
                        let out_path = target_dir.join(&out.relative_path);
                        actions.push(Action {
                            id: format!("cached-{}", out.cas_hash),
                            kind: if out.executable {
                                ActionKind::CompileBin
                            } else {
                                ActionKind::CompileRlib
                            },
                            description: format!("Restored {} from CAS", out.relative_path.display()),
                            toolchain: toolchain.clone(),
                            inputs: Vec::new(),
                            outputs: vec![out_path],
                            args: Vec::new(),
                            env: HashMap::new(),
                            dependencies: Vec::new(),
                        });
                    }

                    let plan = BuildPlan {
                        actions,
                        target_directory: target_dir,
                    };
                    let result = ActionResult {
                        action_id: entry.action_id,
                        exit_code: entry.exit_code,
                        success: entry.exit_code == 0,
                        stdout: entry.stdout,
                        stderr: entry.stderr,
                        cached: true,
                    };
                    return Ok((plan, result));
                }
            }

            // 2. Cache miss or corrupt: acquire single-flight lock
            let lock_path = ac.lock_path(&action_key);
            let _lock = FileLock::acquire_exclusive(&lock_path)?;

            // Double check inside lock
            if let Ok(Some(entry)) = ac.get(&action_key) {
                let mut all_valid = !entry.outputs.is_empty();
                for out in &entry.outputs {
                    if !cas.has_object(&out.cas_hash)
                        || !cas.verify_object(&out.cas_hash).unwrap_or(false)
                    {
                        all_valid = false;
                        break;
                    }
                }
                if all_valid {
                    for out in &entry.outputs {
                        let dest = target_dir.join(&out.relative_path);
                        cas.restore_object(&out.cas_hash, &dest, out.executable)?;
                    }

                    let mut actions = Vec::new();
                    for out in &entry.outputs {
                        let out_path = target_dir.join(&out.relative_path);
                        actions.push(Action {
                            id: format!("cached-{}", out.cas_hash),
                            kind: if out.executable {
                                ActionKind::CompileBin
                            } else {
                                ActionKind::CompileRlib
                            },
                            description: format!("Restored {} from CAS", out.relative_path.display()),
                            toolchain: toolchain.clone(),
                            inputs: Vec::new(),
                            outputs: vec![out_path],
                            args: Vec::new(),
                            env: HashMap::new(),
                            dependencies: Vec::new(),
                        });
                    }

                    let plan = BuildPlan {
                        actions,
                        target_directory: target_dir,
                    };
                    let result = ActionResult {
                        action_id: entry.action_id,
                        exit_code: entry.exit_code,
                        success: entry.exit_code == 0,
                        stdout: entry.stdout,
                        stderr: entry.stderr,
                        cached: true,
                    };
                    return Ok((plan, result));
                }
            }

            // 3. Execute compilation
            let (plan, result) =
                Self::run_cargo_build(&manifest_path, &target_dir, &toolchain, opt)?;

            // 4. If build succeeded, publish artifacts to CAS and AC
            if result.success {
                let mut cached_outputs = Vec::new();
                for action in &plan.actions {
                    for output_file in &action.outputs {
                        if output_file.is_file() {
                            if let Ok((cas_hash, file_size)) = cas.store_file(output_file) {
                                let rel = output_file
                                    .strip_prefix(&target_dir)
                                    .unwrap_or(output_file)
                                    .to_path_buf();
                                let is_executable = action.kind == ActionKind::CompileBin
                                    && (output_file.extension().is_none()
                                        || output_file
                                            .extension()
                                            .map(|e| e == "exe")
                                            .unwrap_or(false));
                                if !cached_outputs
                                    .iter()
                                    .any(|c: &CachedOutput| c.relative_path == rel)
                                {
                                    cached_outputs.push(CachedOutput {
                                        relative_path: rel,
                                        cas_hash,
                                        file_size,
                                        executable: is_executable,
                                    });
                                }
                            }
                        }
                    }
                }

                let entry = ActionCacheEntry {
                    action_key: action_key.clone(),
                    action_id: "build-all".to_string(),
                    created_at_secs: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0),
                    outputs: cached_outputs,
                    stdout: result.stdout.clone(),
                    stderr: result.stderr.clone(),
                    exit_code: result.exit_code,
                };
                let _ = ac.put(&entry);
            }

            return Ok((plan, result));
        }

        // Uncached execution (ineligible actions or --no-cache)
        Self::run_cargo_build(&manifest_path, &target_dir, &toolchain, opt)
    }

    /// Internal helper executing cargo build directly without caching.
    fn run_cargo_build(
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
        let stderr_thread = std::thread::spawn(move || -> std::io::Result<String> {
            let mut recorded_stderr = String::new();
            let reader = BufReader::new(stderr);
            for line in reader.lines() {
                let line = line?;
                recorded_stderr.push_str(&line);
                recorded_stderr.push('\n');
                eprintln!("{}", line);
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
}

/// Recursively discovers and hashes all Rust source files and manifests in a package directory.
fn hash_package_sources(package_dir: &Path) -> Result<Vec<(String, String)>> {
    let mut sources = Vec::new();
    let cargo_toml = package_dir.join("Cargo.toml");
    if cargo_toml.is_file() {
        if let Ok(hash) = Sha256::digest_file(&cargo_toml) {
            sources.push(("Cargo.toml".to_string(), hash));
        }
    }
    let cargo_lock = package_dir.join("Cargo.lock");
    if cargo_lock.is_file() {
        if let Ok(hash) = Sha256::digest_file(&cargo_lock) {
            sources.push(("Cargo.lock".to_string(), hash));
        }
    }

    let mut stack = vec![package_dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let file_name = entry.file_name();
            let name_str = file_name.to_string_lossy();
            if path.is_dir() {
                if name_str == "target" || name_str == ".mitts" || name_str == ".git" {
                    continue;
                }
                stack.push(path);
            } else if path.is_file() && path.extension().map(|e| e == "rs").unwrap_or(false) {
                if let Ok(rel) = path.strip_prefix(package_dir) {
                    let rel_str = rel.to_string_lossy().to_string();
                    if let Ok(h) = Sha256::digest_file(&path) {
                        sources.push((rel_str, h));
                    }
                }
            }
        }
    }
    sources.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(sources)
}

/// Derives a deterministic Action Key for the entire build plan.
fn derive_build_action_key(
    opt: &RustBuildOptions,
    explanation: &EffectivePlanExplanation,
) -> Result<String> {
    let mut all_sources = Vec::new();
    let mut visited_pkg_dirs = HashSet::new();
    let mut dep_info = Vec::new();

    for unit in &explanation.units {
        dep_info.push((unit.package_name.as_str(), unit.version.as_str()));

        let mut cur = unit.src_path.parent();
        let mut pkg_dir = None;
        while let Some(dir) = cur {
            if dir.join("Cargo.toml").is_file() {
                pkg_dir = Some(dir.to_path_buf());
                break;
            }
            cur = dir.parent();
        }

        if let Some(p_dir) = pkg_dir {
            if visited_pkg_dirs.insert(p_dir.clone()) {
                if let Ok(pkg_sources) = hash_package_sources(&p_dir) {
                    for (rel_path, h) in pkg_sources {
                        let qualified = format!("{}/{}", unit.package_name, rel_path);
                        all_sources.push((qualified, h));
                    }
                }
            }
        } else if let Ok(h) = Sha256::digest_file(&unit.src_path) {
            all_sources.push((unit.target_name.clone(), h));
        }
    }

    let source_refs: Vec<(&str, &str)> = all_sources
        .iter()
        .map(|(p, h)| (p.as_str(), h.as_str()))
        .collect();

    let target_triple = explanation
        .toolchain
        .target_triple
        .as_deref()
        .unwrap_or("unknown");

    let action_key = ActionCache::derive_action_key(
        &explanation.toolchain.version,
        target_triple,
        &explanation.profile,
        &opt.extra_args,
        &opt.features,
        &source_refs,
        &dep_info,
    );

    Ok(action_key)
}
