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
    #[serde(default)]
    pub package_id: String,
    pub package_name: String,
    pub version: String,
    pub target_name: String,
    pub kind: String,
    pub src_path: PathBuf,
    #[serde(default)]
    pub manifest_path: PathBuf,
    pub cache_eligibility: String,
    #[serde(default)]
    pub dependencies: Vec<String>,
}

impl PlannedCompilationUnit {
    pub fn unit_id(&self) -> String {
        format!("{}::{}:{}", self.package_id, self.kind, self.target_name)
    }
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

                    let mut unit_deps = Vec::new();
                    if let Some(ref resolve) = meta.resolve {
                        if let Some(node) = resolve.nodes.iter().find(|n| n.id == pkg.id) {
                            for dep_id in &node.dependencies {
                                if let Some(dep_pkg) = packages_by_id.get(dep_id) {
                                    for dep_target in &dep_pkg.targets {
                                        if dep_target.kind.iter().any(|k| k == "lib" || k == "rlib") {
                                            let dep_kind = if dep_target.kind.iter().any(|k| k == "bin") {
                                                "bin"
                                            } else {
                                                "lib"
                                            };
                                            let dep_unit_id = format!("{}::{}:{}", dep_pkg.id, dep_kind, dep_target.name);
                                            unit_deps.push(dep_unit_id);
                                        }
                                    }
                                }
                            }
                        }
                    }
                    if is_bin {
                        for other_target in &pkg.targets {
                            if other_target.name != target.name
                                && other_target.kind.iter().any(|k| k == "lib" || k == "rlib")
                            {
                                let dep_kind = "lib";
                                let dep_unit_id = format!("{}::{}:{}", pkg.id, dep_kind, other_target.name);
                                unit_deps.push(dep_unit_id);
                            }
                        }
                    }
                    unit_deps.sort();
                    unit_deps.dedup();

                    units.push(PlannedCompilationUnit {
                        package_id: pkg.id.clone(),
                        package_name: pkg.name.clone(),
                        version: pkg.version.clone(),
                        target_name: target.name.clone(),
                        kind: kind_str,
                        src_path: PathBuf::from(&target.src_path),
                        manifest_path: PathBuf::from(&pkg.manifest_path),
                        cache_eligibility: eligibility,
                        dependencies: unit_deps,
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

    /// Derives deterministic Action Keys for each compilation unit in the plan.
    pub fn derive_unit_action_keys(
        opt: &RustBuildOptions,
        explanation: &EffectivePlanExplanation,
    ) -> Result<HashMap<String, String>> {
        derive_unit_action_keys(opt, explanation)
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
            if let Ok(unit_keys) = derive_unit_action_keys(opt, &explanation) {
                let cas = match opt.cache_dir {
                    Some(ref cd) => ContentAddressedStore::new(cd.join("cas"))?,
                    None => ContentAddressedStore::default_store()?,
                };
                let ac = match opt.cache_dir {
                    Some(ref cd) => ActionCache::new(cd.join("ac"))?,
                    None => ActionCache::default_cache()?,
                };

                // Coordinate target directory access to prevent concurrent workers racing on target_dir
                let target_lock_hash = Sha256::digest(target_dir.to_string_lossy().as_bytes());
                let target_lock_path = ac.locks_dir().join(format!("target_{}.lock", target_lock_hash));
                let _target_lock = FileLock::acquire_exclusive(&target_lock_path)?;

                // 1. Initial check: evaluate action cache for each compilation unit
                let mut hit_units = Vec::new();
                let mut missed_units = Vec::new();

                for unit in &explanation.units {
                    let u_id = unit.unit_id();
                    if let Some(key) = unit_keys.get(&u_id) {
                        if let Ok(Some(entry)) = ac.get(key) {
                            let mut valid = !entry.outputs.is_empty();
                            for out in &entry.outputs {
                                if !cas.has_object(&out.cas_hash)
                                    || !cas.verify_object(&out.cas_hash).unwrap_or(false)
                                {
                                    valid = false;
                                    break;
                                }
                            }
                            if valid {
                                hit_units.push((unit, entry));
                                continue;
                            }
                        }
                    }
                    missed_units.push(unit);
                }

                // If ALL units hit: restore all and return immediately without running Cargo
                if missed_units.is_empty() && hit_units.len() == explanation.units.len() {
                    let mut actions = Vec::new();
                    for (_unit, entry) in hit_units {
                        let mut outputs = Vec::new();
                        let mut is_bin = false;
                        for out in &entry.outputs {
                            let dest = target_dir.join(&out.relative_path);
                            cas.restore_object(&out.cas_hash, &dest, out.executable)?;
                            if out.executable {
                                is_bin = true;
                            }
                            outputs.push(dest);
                        }
                        actions.push(Action {
                            id: entry.action_id.clone(),
                            kind: if is_bin {
                                ActionKind::CompileBin
                            } else {
                                ActionKind::CompileRlib
                            },
                            description: format!("Restored {} from CAS", entry.action_id),
                            toolchain: toolchain.clone(),
                            inputs: Vec::new(),
                            outputs,
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
                        action_id: "build-all".to_string(),
                        exit_code: 0,
                        success: true,
                        stdout: String::new(),
                        stderr: String::new(),
                        cached: true,
                    };
                    return Ok((plan, result));
                }

                // 2. Single-flight coordination across active action keys:
                // Acquire exclusive locks for ALL missed action units in canonical sorted order.
                // This eliminates duplicate compilation of shared dependencies across separate workspaces.
                let mut missed_keys: Vec<String> = missed_units
                    .iter()
                    .map(|u| unit_keys[&u.unit_id()].clone())
                    .collect();
                missed_keys.sort();
                missed_keys.dedup();

                let mut _action_locks = Vec::new();
                for key in &missed_keys {
                    let lock_path = ac.lock_path(key);
                    _action_locks.push(FileLock::acquire_exclusive(&lock_path)?);
                }

                // Double check inside locks in case concurrent workers finished missed units while we waited
                let mut locked_hit_units = Vec::new();
                let mut locked_missed_units = Vec::new();

                for unit in &explanation.units {
                    let u_id = unit.unit_id();
                    if let Some(key) = unit_keys.get(&u_id) {
                        if let Ok(Some(entry)) = ac.get(key) {
                            let mut valid = !entry.outputs.is_empty();
                            for out in &entry.outputs {
                                if !cas.has_object(&out.cas_hash)
                                    || !cas.verify_object(&out.cas_hash).unwrap_or(false)
                                {
                                    valid = false;
                                    break;
                                }
                            }
                            if valid {
                                locked_hit_units.push((unit, entry));
                                continue;
                            }
                        }
                    }
                    locked_missed_units.push(unit);
                }

                if locked_missed_units.is_empty() && locked_hit_units.len() == explanation.units.len() {
                    let mut actions = Vec::new();
                    for (_unit, entry) in locked_hit_units {
                        let mut outputs = Vec::new();
                        let mut is_bin = false;
                        for out in &entry.outputs {
                            let dest = target_dir.join(&out.relative_path);
                            cas.restore_object(&out.cas_hash, &dest, out.executable)?;
                            if out.executable {
                                is_bin = true;
                            }
                            outputs.push(dest);
                        }
                        actions.push(Action {
                            id: entry.action_id.clone(),
                            kind: if is_bin {
                                ActionKind::CompileBin
                            } else {
                                ActionKind::CompileRlib
                            },
                            description: format!("Restored {} from CAS", entry.action_id),
                            toolchain: toolchain.clone(),
                            inputs: Vec::new(),
                            outputs,
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
                        action_id: "build-all".to_string(),
                        exit_code: 0,
                        success: true,
                        stdout: String::new(),
                        stderr: String::new(),
                        cached: true,
                    };
                    return Ok((plan, result));
                }

                // 3. Partial hits: Pre-restore valid cached dependency artifacts into target directory
                // so Cargo reuses them and does not rebuild already-cached units!
                for (_unit, entry) in &locked_hit_units {
                    for out in &entry.outputs {
                        let dest = target_dir.join(&out.relative_path);
                        let _ = cas.restore_object(&out.cas_hash, &dest, out.executable);
                    }
                }

                // 4. Execute compilation for missed units
                let (plan, result) =
                    Self::run_cargo_build(&manifest_path, &target_dir, &toolchain, opt)?;

                // 5. Publish artifacts per action unit ONLY if ALL outputs for that action exist and store cleanly
                if result.success {
                    for action in &plan.actions {
                        let matching_unit = explanation.units.iter().find(|u| u.unit_id() == action.id);

                        if let Some(unit) = matching_unit {
                            let u_id = unit.unit_id();
                            if let Some(action_key) = unit_keys.get(&u_id) {
                                if action.outputs.is_empty() {
                                    continue;
                                }

                                let mut stored_outputs = Vec::new();
                                let mut all_outputs_stored = true;

                                for output_file in &action.outputs {
                                    if !output_file.is_file() {
                                        all_outputs_stored = false;
                                        break;
                                    }

                                    match cas.store_file(output_file) {
                                        Ok((cas_hash, file_size)) => {
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
                                            if !stored_outputs
                                                .iter()
                                                .any(|c: &CachedOutput| c.relative_path == rel)
                                            {
                                                stored_outputs.push(CachedOutput {
                                                    relative_path: rel,
                                                    cas_hash,
                                                    file_size,
                                                    executable: is_executable,
                                                });
                                            }
                                        }
                                        Err(_) => {
                                            all_outputs_stored = false;
                                            break;
                                        }
                                    }
                                }

                                // Also capture .fingerprint files for this package if present
                                let pkg_name_clean = &unit.package_name;
                                let pkg_name_norm = pkg_name_clean.replace('-', "_");
                                let fp_dir = target_dir.join(&explanation.profile).join(".fingerprint");
                                if fp_dir.is_dir() {
                                    if let Ok(entries) = fs::read_dir(&fp_dir) {
                                        for entry in entries.flatten() {
                                            let path = entry.path();
                                            let fname = entry.file_name();
                                            let fname_str = fname.to_string_lossy();
                                            if fname_str.starts_with(&format!("{}-", pkg_name_clean))
                                                || fname_str.starts_with(&format!("{}-", pkg_name_norm))
                                                || fname_str.starts_with(&format!("lib{}-", pkg_name_norm))
                                            {
                                                if path.is_dir() {
                                                    if let Ok(sub_entries) = fs::read_dir(&path) {
                                                        for sub in sub_entries.flatten() {
                                                            let sub_path = sub.path();
                                                            if sub_path.is_file() {
                                                                if let Ok((cas_hash, file_size)) = cas.store_file(&sub_path) {
                                                                    let rel = sub_path
                                                                        .strip_prefix(&target_dir)
                                                                        .unwrap_or(&sub_path)
                                                                        .to_path_buf();
                                                                    stored_outputs.push(CachedOutput {
                                                                        relative_path: rel,
                                                                        cas_hash,
                                                                        file_size,
                                                                        executable: false,
                                                                    });
                                                                }
                                                            }
                                                        }
                                                    }
                                                } else if path.is_file() {
                                                    if let Ok((cas_hash, file_size)) = cas.store_file(&path) {
                                                        let rel = path
                                                            .strip_prefix(&target_dir)
                                                            .unwrap_or(&path)
                                                            .to_path_buf();
                                                        stored_outputs.push(CachedOutput {
                                                            relative_path: rel,
                                                            cas_hash,
                                                            file_size,
                                                            executable: false,
                                                        });
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }

                                // Publish ONLY if EVERY required output was captured and stored successfully
                                if all_outputs_stored && !stored_outputs.is_empty() {
                                    let entry = ActionCacheEntry {
                                        action_key: action_key.clone(),
                                        action_id: unit.unit_id(),
                                        created_at_secs: std::time::SystemTime::now()
                                            .duration_since(std::time::UNIX_EPOCH)
                                            .map(|d| d.as_secs())
                                            .unwrap_or(0),
                                        outputs: stored_outputs,
                                        stdout: String::new(),
                                        stderr: String::new(),
                                        exit_code: 0,
                                    };
                                    let _ = ac.put(&entry);
                                }
                            }
                        }
                    }
                }

                return Ok((plan, result));
            }
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
                        let (kind, kind_str) = if target.kind.iter().any(|k| k == "bin") {
                            (ActionKind::CompileBin, "bin")
                        } else if target.kind.iter().any(|k| k == "lib" || k == "rlib") {
                            (ActionKind::CompileRlib, "lib")
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

/// Discovers all matching dep-info (.d) files produced by rustc/cargo for the target.
fn find_dep_info_files(target_dir: &Path, target_name: &str) -> Vec<PathBuf> {
    let mut d_files = Vec::new();
    let norm_target = target_name.replace('-', "_");
    for profile in &["debug", "release"] {
        let p_dir = target_dir.join(profile);
        if !p_dir.is_dir() {
            continue;
        }
        let direct = p_dir.join(format!("{}.d", target_name));
        if direct.is_file() {
            d_files.push(direct);
        }
        let direct_norm = p_dir.join(format!("{}.d", norm_target));
        if direct_norm.is_file() && direct_norm != p_dir.join(format!("{}.d", target_name)) {
            d_files.push(direct_norm);
        }
        let deps_dir = p_dir.join("deps");
        if deps_dir.is_dir() {
            if let Ok(entries) = fs::read_dir(&deps_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().map(|e| e == "d").unwrap_or(false) {
                        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                            let matches = stem == target_name
                                || stem == norm_target
                                || stem.starts_with(&format!("{}-", target_name))
                                || stem.starts_with(&format!("{}-", norm_target))
                                || stem.starts_with(&format!("lib{}-", norm_target))
                                || stem.starts_with(&format!("lib{}-", target_name));
                            if matches {
                                d_files.push(path);
                            }
                        }
                    }
                }
            }
        }
    }
    d_files
}

/// Parses a Makefile-style dep-info (.d) file produced by rustc to extract all included paths.
fn parse_dep_info(d_path: &Path) -> Result<Vec<PathBuf>> {
    let content = fs::read_to_string(d_path)
        .with_context(|| format!("Failed to read dep-info file {:?}", d_path))?;
    let mut files = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        let tokens_part = if let Some((_, deps_part)) = line.split_once(':') {
            deps_part
        } else {
            line
        };
        for token in tokens_part.split_whitespace() {
            let token = token.trim_end_matches('\\');
            if !token.is_empty() {
                let p = PathBuf::from(token);
                if p.is_file() {
                    files.push(p);
                }
            }
        }
    }
    Ok(files)
}

/// Gathers and hashes all effective compile-time inputs for a specific unit:
/// - Cargo.toml and Cargo.lock
/// - Applicable .cargo/config.toml or .cargo/config files
/// - Global ~/.cargo/config.toml / $CARGO_HOME configs if present
/// - All source and asset files in the package directory
/// - Tracked dep-info (.d) files from rustc (including include_bytes! / include_str!)
///
/// Returns error on any read/hash failure to prevent publishing invalid cache keys.
fn hash_unit_inputs(
    package_dir: &Path,
    workspace_manifest: &Path,
    target_dir: &Path,
    target_name: &str,
) -> Result<Vec<(String, String)>> {
    let mut inputs = Vec::new();

    // 1. Mandatory package manifest
    let cargo_toml = package_dir.join("Cargo.toml");
    if cargo_toml.is_file() {
        let hash = Sha256::digest_file(&cargo_toml)
            .with_context(|| format!("Failed to hash {:?}", cargo_toml))?;
        inputs.push(("Cargo.toml".to_string(), hash));
    } else {
        bail!("Cargo.toml not found in {:?}", package_dir);
    }

    // 2. Workspace root manifest (and any ancestor manifests between package_dir and filesystem root)
    if workspace_manifest.is_file() && workspace_manifest != cargo_toml {
        let hash = Sha256::digest_file(workspace_manifest)
            .with_context(|| format!("Failed to hash workspace manifest {:?}", workspace_manifest))?;
        inputs.push((format!("workspace:{}", workspace_manifest.display()), hash));
    }

    let mut ancestor_candidate = package_dir.parent();
    while let Some(dir) = ancestor_candidate {
        let toml_path = dir.join("Cargo.toml");
        if toml_path.is_file() && toml_path != cargo_toml && toml_path != workspace_manifest {
            let hash = Sha256::digest_file(&toml_path)
                .with_context(|| format!("Failed to hash ancestor manifest {:?}", toml_path))?;
            inputs.push((format!("ancestor:{}", toml_path.display()), hash));
        }
        ancestor_candidate = dir.parent();
    }

    // 2. Lockfile in package directory or ancestors
    let mut lock_candidate = Some(package_dir);
    while let Some(dir) = lock_candidate {
        let lock_path = dir.join("Cargo.lock");
        if lock_path.is_file() {
            let hash = Sha256::digest_file(&lock_path)
                .with_context(|| format!("Failed to hash {:?}", lock_path))?;
            inputs.push(("Cargo.lock".to_string(), hash));
            break;
        }
        lock_candidate = dir.parent();
    }

    // 3. Applicable .cargo/config.toml or .cargo/config in package dir or ancestor dirs
    let mut config_candidate = Some(package_dir);
    while let Some(dir) = config_candidate {
        let toml_cfg = dir.join(".cargo").join("config.toml");
        if toml_cfg.is_file() {
            let hash = Sha256::digest_file(&toml_cfg)
                .with_context(|| format!("Failed to hash {:?}", toml_cfg))?;
            inputs.push((format!("{}/.cargo/config.toml", dir.display()), hash));
        }
        let bare_cfg = dir.join(".cargo").join("config");
        if bare_cfg.is_file() {
            let hash = Sha256::digest_file(&bare_cfg)
                .with_context(|| format!("Failed to hash {:?}", bare_cfg))?;
            inputs.push((format!("{}/.cargo/config", dir.display()), hash));
        }
        config_candidate = dir.parent();
    }

    // 4. Global Cargo configs in $CARGO_HOME or $HOME/.cargo
    if let Ok(cargo_home) = std::env::var("CARGO_HOME") {
        let home_path = PathBuf::from(cargo_home);
        for name in &["config.toml", "config"] {
            let p = home_path.join(name);
            if p.is_file() {
                let hash = Sha256::digest_file(&p)
                    .with_context(|| format!("Failed to hash {:?}", p))?;
                inputs.push((format!("CARGO_HOME/{}", name), hash));
            }
        }
    } else if let Ok(home) = std::env::var("HOME") {
        let cargo_dir = PathBuf::from(home).join(".cargo");
        for name in &["config.toml", "config"] {
            let p = cargo_dir.join(name);
            if p.is_file() {
                let hash = Sha256::digest_file(&p)
                    .with_context(|| format!("Failed to hash {:?}", p))?;
                inputs.push((format!("~/.cargo/{}", name), hash));
            }
        }
    }

    // 5. Check if rustc .d (dep-info) file exists for tracked compile-time includes
    for d_path in find_dep_info_files(target_dir, target_name) {
        let tracked_files = parse_dep_info(&d_path)?;
        for p in tracked_files {
            let hash = Sha256::digest_file(&p)
                .with_context(|| format!("Failed to hash tracked dep-info file {:?}", p))?;
            let rel = p
                .strip_prefix(package_dir)
                .unwrap_or(&p)
                .to_string_lossy()
                .to_string();
            inputs.push((rel, hash));
        }
    }

    // 6. Recursive scan of package directory (src, assets, templates, etc.)
    let mut stack = vec![package_dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = fs::read_dir(&dir)
            .with_context(|| format!("Failed to read directory {:?}", dir))?;
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            let file_name = entry.file_name();
            let name_str = file_name.to_string_lossy();
            if path.is_dir() {
                if name_str == "target" || name_str == ".mitts" || name_str == ".git" {
                    continue;
                }
                stack.push(path);
            } else if path.is_file() {
                // Ignore transient OS metadata files (e.g. .DS_Store)
                if name_str.starts_with('.') && !name_str.ends_with(".toml") && !name_str.ends_with(".lock") {
                    continue;
                }
                let hash = Sha256::digest_file(&path)
                    .with_context(|| format!("Failed to hash package file {:?}", path))?;
                let rel = path
                    .strip_prefix(package_dir)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .to_string();
                inputs.push((rel, hash));
            }
        }
    }

    inputs.sort_by(|a, b| a.0.cmp(&b.0));
    inputs.dedup_by(|a, b| a.0 == b.0);
    Ok(inputs)
}

/// Resolves the effective target triple considering CLI args, Cargo env vars, and toolchain defaults.
fn resolve_effective_target_triple(
    opt: &RustBuildOptions,
    toolchain: &ToolchainInfo,
) -> String {
    // 1. Check CLI args: --target <val> or --target=<val>
    let mut args_iter = opt.extra_args.iter();
    while let Some(arg) = args_iter.next() {
        if arg == "--target" {
            if let Some(val) = args_iter.next() {
                return val.clone();
            }
        } else if let Some(val) = arg.strip_prefix("--target=") {
            return val.to_string();
        }
    }

    // 2. Check CARGO_BUILD_TARGET environment variable
    if let Ok(target) = std::env::var("CARGO_BUILD_TARGET") {
        if !target.is_empty() {
            return target;
        }
    }

    // 3. Fallback to detected toolchain target triple (host)
    toolchain
        .target_triple
        .clone()
        .unwrap_or_else(|| "unknown".to_string())
}

/// Derives independent deterministic Action Keys for each compilation unit in topological order.
fn derive_unit_action_keys(
    opt: &RustBuildOptions,
    explanation: &EffectivePlanExplanation,
) -> Result<HashMap<String, String>> {
    let mut unit_keys: HashMap<String, String> = HashMap::new();
    let mut inputs_by_unit: HashMap<String, Vec<(String, String)>> = HashMap::new();

    // Gather effective environment flags (all CARGO_*, RUST_*, RUSTFLAGS, etc.)
    let mut effective_flags = opt.extra_args.clone();
    let mut relevant_env_keys: Vec<String> = std::env::vars()
        .filter_map(|(k, _)| {
            if k.starts_with("CARGO_")
                || k.starts_with("RUST_")
                || k == "RUSTFLAGS"
                || k == "RUSTC_WRAPPER"
                || k == "RUSTC_WORKSPACE_WRAPPER"
                || k == "RUSTDOCFLAGS"
                || k == "TARGET"
                || k == "HOST"
                || k == "PROFILE"
            {
                Some(k)
            } else {
                None
            }
        })
        .collect();
    relevant_env_keys.sort();
    for k in relevant_env_keys {
        if let Ok(v) = std::env::var(&k) {
            effective_flags.push(format!("ENV:{}={}", k, v));
        }
    }

    let mut flags_by_unit: HashMap<String, Vec<String>> = HashMap::new();

    for unit in &explanation.units {
        let pkg_dir = if unit.manifest_path.is_file() {
            unit.manifest_path.parent().unwrap().to_path_buf()
        } else {
            let mut cur = unit.src_path.parent();
            let mut dir = None;
            while let Some(d) = cur {
                if d.join("Cargo.toml").is_file() {
                    dir = Some(d.to_path_buf());
                    break;
                }
                cur = d.parent();
            }
            dir.unwrap_or_else(|| {
                unit.src_path.parent().unwrap_or(&unit.src_path).to_path_buf()
            })
        };

        let inputs = hash_unit_inputs(
            &pkg_dir,
            &explanation.manifest_path,
            &explanation.target_directory,
            &unit.target_name,
        )?;
        inputs_by_unit.insert(unit.unit_id(), inputs);

        // Include Cargo-generated compiler environment variables for this unit:
        let mut u_flags = effective_flags.clone();
        u_flags.push(format!("CARGO_MANIFEST_DIR={}", pkg_dir.display()));
        u_flags.push(format!("CARGO_PKG_NAME={}", unit.package_name));
        u_flags.push(format!("CARGO_PKG_VERSION={}", unit.version));
        u_flags.push(format!("CARGO_CRATE_NAME={}", unit.target_name));
        if unit.kind == "bin" {
            u_flags.push(format!("CARGO_BIN_NAME={}", unit.target_name));
        }
        flags_by_unit.insert(unit.unit_id(), u_flags);
    }

    let planned_unit_ids: HashSet<String> = explanation
        .units
        .iter()
        .map(|u| u.unit_id())
        .collect();

    let mut remaining: Vec<&PlannedCompilationUnit> = explanation.units.iter().collect();
    let target_triple = resolve_effective_target_triple(opt, &explanation.toolchain);

    while !remaining.is_empty() {
        let mut progress = false;
        let mut next_remaining = Vec::new();

        for unit in remaining {
            let u_id = unit.unit_id();
            let all_deps_ready = unit
                .dependencies
                .iter()
                .filter(|dep_id| planned_unit_ids.contains(dep_id.as_str()))
                .all(|dep_id| unit_keys.contains_key(dep_id));

            if all_deps_ready {
                let unit_inputs = &inputs_by_unit[&u_id];
                let source_refs: Vec<(&str, &str)> = unit_inputs
                    .iter()
                    .map(|(p, h)| (p.as_str(), h.as_str()))
                    .collect();

                let mut dep_hashes = Vec::new();
                for dep_id in &unit.dependencies {
                    if let Some(dep_key) = unit_keys.get(dep_id) {
                        dep_hashes.push((dep_id.as_str(), dep_key.as_str()));
                    }
                }
                dep_hashes.sort();

                let u_flags = &flags_by_unit[&u_id];
                let key = ActionCache::derive_action_key(
                    &explanation.toolchain.version,
                    &target_triple,
                    &explanation.profile,
                    u_flags,
                    &opt.features,
                    &source_refs,
                    &dep_hashes,
                );

                unit_keys.insert(u_id, key);
                progress = true;
            } else {
                next_remaining.push(unit);
            }
        }

        if !progress {
            for unit in next_remaining {
                let u_id = unit.unit_id();
                let unit_inputs = &inputs_by_unit[&u_id];
                let source_refs: Vec<(&str, &str)> = unit_inputs
                    .iter()
                    .map(|(p, h)| (p.as_str(), h.as_str()))
                    .collect();
                let u_flags = &flags_by_unit[&u_id];
                let key = ActionCache::derive_action_key(
                    &explanation.toolchain.version,
                    &target_triple,
                    &explanation.profile,
                    u_flags,
                    &opt.features,
                    &source_refs,
                    &[],
                );
                unit_keys.insert(u_id, key);
            }
            break;
        }

        remaining = next_remaining;
    }

    Ok(unit_keys)
}
