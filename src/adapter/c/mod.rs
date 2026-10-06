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
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

#[derive(Debug, Clone, Default)]
pub struct CBuildOptions {
    pub manifest_path: Option<PathBuf>,
    pub target_dir: Option<PathBuf>,
    pub cache_dir: Option<PathBuf>,
    pub release: bool,
    pub no_cache: bool,
    pub extra_flags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CPackageManifest {
    pub package: CPackageInfo,
    #[serde(default)]
    pub bin: Option<CTargetInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CPackageInfo {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CTargetInfo {
    pub src: String,
}

pub struct CAdapter;

impl CAdapter {
    /// Detects host C compiler (e.g. clang or gcc or cc).
    pub fn detect_toolchain() -> Result<ToolchainInfo> {
        let output = Command::new("cc")
            .arg("--version")
            .output()
            .context("Failed to invoke 'cc --version'")?;

        let version = String::from_utf8_lossy(&output.stdout)
            .lines()
            .next()
            .unwrap_or("cc")
            .trim()
            .to_string();

        Ok(ToolchainInfo {
            name: "cc".to_string(),
            version,
            target_triple: None,
        })
    }

    /// Resolves the C project manifest file (`Mitts.toml`).
    pub fn resolve_manifest(opt: &CBuildOptions) -> Result<PathBuf> {
        if let Some(ref p) = opt.manifest_path {
            if p.is_file() {
                return Ok(p.clone());
            }
            let default_name = p.join("Mitts.toml");
            if default_name.is_file() {
                return Ok(default_name);
            }
            bail!("C manifest not found at {:?}", p);
        }
        let cur = std::env::current_dir()?.join("Mitts.toml");
        if cur.is_file() {
            return Ok(cur);
        }
        bail!("Mitts.toml not found in current directory");
    }

    /// Parses a C package manifest from TOML content.
    pub fn parse_manifest(content: &str) -> Result<CPackageManifest> {
        // Simple line parser for [package] and [bin]
        let mut pkg_name = String::new();
        let mut pkg_version = String::new();
        let mut bin_src = String::new();
        let mut section = "";

        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            if trimmed == "[package]" {
                section = "package";
                continue;
            } else if trimmed == "[bin]" {
                section = "bin";
                continue;
            }

            if let Some((k, v)) = trimmed.split_once('=') {
                let key = k.trim();
                let val = v.trim().trim_matches('"');
                if section == "package" {
                    if key == "name" {
                        pkg_name = val.to_string();
                    } else if key == "version" {
                        pkg_version = val.to_string();
                    }
                } else if section == "bin" && key == "src" {
                    bin_src = val.to_string();
                }
            }
        }

        if pkg_name.is_empty() {
            bail!("Manifest missing package name");
        }

        Ok(CPackageManifest {
            package: CPackageInfo {
                name: pkg_name,
                version: if pkg_version.is_empty() {
                    "0.1.0".to_string()
                } else {
                    pkg_version
                },
            },
            bin: if bin_src.is_empty() {
                None
            } else {
                Some(CTargetInfo { src: bin_src })
            },
        })
    }

    /// Builds a C project using the shared CAS core and Action Cache.
    pub fn build(opt: &CBuildOptions) -> Result<(BuildPlan, ActionResult)> {
        let manifest_path = Self::resolve_manifest(opt)?;
        let manifest_dir = manifest_path.parent().unwrap();
        let manifest_content = fs::read_to_string(&manifest_path)?;
        let manifest = Self::parse_manifest(&manifest_content)?;

        let toolchain = Self::detect_toolchain()?;

        let target_dir = match opt.target_dir {
            Some(ref td) => td.clone(),
            None => manifest_dir.join(".mitts").join("target"),
        };
        let profile_dir = target_dir.join(if opt.release { "release" } else { "debug" });
        fs::create_dir_all(&profile_dir)?;

        let bin_info = manifest.bin.context("C package has no [bin] target")?;
        let src_file = manifest_dir.join(&bin_info.src);
        if !src_file.is_file() {
            bail!("C source file not found at {:?}", src_file);
        }

        let src_hash = Sha256::digest_file(&src_file)?;
        let dest_bin = profile_dir.join(&manifest.package.name);

        let cas = match opt.cache_dir {
            Some(ref cd) => ContentAddressedStore::new(cd.join("cas"))?,
            None => ContentAddressedStore::default_store()?,
        };
        let ac = match opt.cache_dir {
            Some(ref cd) => ActionCache::new(cd.join("ac"))?,
            None => ActionCache::default_cache()?,
        };

        // Derive Action Key
        let action_key = ActionCache::derive_action_key(
            &toolchain.version,
            "c-target",
            if opt.release { "release" } else { "debug" },
            &opt.extra_flags,
            &[],
            &[(&bin_info.src, &src_hash)],
            &[],
        );

        // Check Action Cache
        if !opt.no_cache {
            if let Ok(Some(entry)) = ac.get(&action_key) {
                if let Some(out) = entry.outputs.first() {
                    if cas.has_object(&out.cas_hash)
                        && cas.verify_object(&out.cas_hash).unwrap_or(false)
                    {
                        cas.restore_object(&out.cas_hash, &dest_bin, true)?;
                        let plan = BuildPlan {
                            actions: vec![Action {
                                id: format!("c::bin:{}", manifest.package.name),
                                kind: ActionKind::CompileBin,
                                description: format!("Restored {} from CAS", manifest.package.name),
                                toolchain,
                                inputs: vec![src_file],
                                outputs: vec![dest_bin],
                                args: Vec::new(),
                                env: HashMap::new(),
                                dependencies: Vec::new(),
                            }],
                            target_directory: target_dir,
                        };
                        let result = ActionResult {
                            action_id: format!("c::bin:{}", manifest.package.name),
                            exit_code: 0,
                            success: true,
                            stdout: String::new(),
                            stderr: String::new(),
                            cached: true,
                        };
                        return Ok((plan, result));
                    }
                }
            }
        }

        // Single-flight lock
        let lock_path = ac.lock_path(&action_key);
        let _lock = FileLock::acquire_exclusive(&lock_path)?;

        // Execute CC compilation
        let mut cmd = Command::new("cc");
        cmd.arg(&src_file);
        if opt.release {
            cmd.arg("-O3");
        } else {
            cmd.arg("-g");
        }
        for flag in &opt.extra_flags {
            cmd.arg(flag);
        }
        cmd.arg("-o").arg(&dest_bin);

        let output = cmd.output().context("Failed to invoke 'cc'")?;
        let success = output.status.success();
        let exit_code = output.status.code().unwrap_or(1);
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        if !success {
            bail!("C compilation failed with exit code {}:\n{}", exit_code, stderr);
        }

        // Store output into CAS
        let (cas_hash, file_size) = cas.store_file(&dest_bin)?;

        if !opt.no_cache {
            let rel_path = PathBuf::from(format!(
                "{}/{}",
                if opt.release { "release" } else { "debug" },
                manifest.package.name
            ));
            let entry = ActionCacheEntry {
                action_key: action_key.clone(),
                action_id: format!("c::bin:{}", manifest.package.name),
                created_at_secs: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
                outputs: vec![CachedOutput {
                    relative_path: rel_path,
                    cas_hash,
                    file_size,
                    executable: true,
                }],
                stdout: stdout.clone(),
                stderr: stderr.clone(),
                exit_code: 0,
            };
            let _ = ac.put(&entry);
        }

        let plan = BuildPlan {
            actions: vec![Action {
                id: format!("c::bin:{}", manifest.package.name),
                kind: ActionKind::CompileBin,
                description: format!("Compiled {} via cc", manifest.package.name),
                toolchain,
                inputs: vec![src_file],
                outputs: vec![dest_bin],
                args: Vec::new(),
                env: HashMap::new(),
                dependencies: Vec::new(),
            }],
            target_directory: target_dir,
        };

        let result = ActionResult {
            action_id: format!("c::bin:{}", manifest.package.name),
            exit_code: 0,
            success: true,
            stdout,
            stderr,
            cached: false,
        };

        Ok((plan, result))
    }
}
