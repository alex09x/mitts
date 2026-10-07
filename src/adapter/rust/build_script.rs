/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::adapter::rust::manifest::{CargoToml, TomlBuild};
use crate::adapter::rust::types::PlannedCompilationUnit;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Output captured from running a `build.rs` build script.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BuildScriptDirectives {
    pub cfgs: Vec<String>,
    pub link_libs: Vec<String>,
    pub link_searches: Vec<String>,
    pub envs: HashMap<String, String>,
    pub out_dir: Option<PathBuf>,
    #[serde(default)]
    pub metadata: HashMap<String, String>,
    #[serde(default)]
    pub links: Option<String>,
}

impl BuildScriptDirectives {
    /// Returns true if all directive collections are empty.
    pub fn is_empty(&self) -> bool {
        self.cfgs.is_empty()
            && self.link_libs.is_empty()
            && self.link_searches.is_empty()
            && self.envs.is_empty()
            && self.metadata.is_empty()
    }

    /// Generates DEP_<LINKS>_<KEY>=<VALUE> environment variables from this build script's metadata.
    pub fn dep_env_vars(&self) -> HashMap<String, String> {
        let mut vars = HashMap::new();
        if let Some(ref links) = self.links {
            let prefix = format!("DEP_{}_", links.to_uppercase().replace('-', "_"));
            for (k, v) in &self.metadata {
                let key = format!("{}{}", prefix, k.to_uppercase().replace('-', "_"));
                vars.insert(key, v.clone());
            }
        }
        vars
    }

    /// Parses cargo: build script directives from raw stdout.
    pub fn parse_stdout(stdout: &str) -> Self {
        let mut directives = Self::default();
        for line in stdout.lines() {
            let trimmed = line.trim();
            let directive = if let Some(rest) = trimmed.strip_prefix("cargo::") {
                rest
            } else if let Some(rest) = trimmed.strip_prefix("cargo:") {
                rest
            } else {
                continue;
            };

            if let Some(rest) = directive.strip_prefix("rustc-cfg=") {
                directives.cfgs.push(rest.to_string());
            } else if let Some(rest) = directive.strip_prefix("rustc-link-lib=") {
                directives.link_libs.push(rest.to_string());
            } else if let Some(rest) = directive.strip_prefix("rustc-link-search=") {
                let val = rest.strip_prefix("native=").unwrap_or(rest);
                directives.link_searches.push(val.to_string());
            } else if let Some(rest) = directive.strip_prefix("rustc-env=") {
                if let Some((k, v)) = rest.split_once('=') {
                    directives.envs.insert(k.to_string(), v.to_string());
                }
            } else {
                let effective = directive.strip_prefix("metadata=").unwrap_or(directive);
                if let Some((k, v)) = effective.split_once('=') {
                    if !k.starts_with("rerun-")
                        && k != "warning"
                        && k != "rustc-check-cfg"
                        && k != "rustc-flags"
                    {
                        directives.metadata.insert(k.to_string(), v.to_string());
                    }
                }
            }
        }
        directives
    }
}

/// Executes a crate's `build.rs` build script in an isolated sandbox.
pub fn execute_build_script(
    unit: &PlannedCompilationUnit,
    staging_dir: &Path,
    release: bool,
    features: &[String],
    cas_artifacts_dir: Option<&Path>,
    extern_crates: &[(String, PathBuf)],
    extra_env: Option<&HashMap<String, String>>,
) -> Result<BuildScriptDirectives> {
    let pkg_dir = unit.manifest_path.parent().unwrap_or(Path::new("."));
    let pkg_toml = CargoToml::from_file(&unit.manifest_path).unwrap_or_default();
    let build_rs = match pkg_toml.package.as_ref().and_then(|p| p.build.as_ref()) {
        Some(TomlBuild::Disabled(false)) => return Ok(BuildScriptDirectives::default()),
        Some(TomlBuild::Path(ref rel_p)) => pkg_dir.join(rel_p),
        _ => {
            let standard = pkg_dir.join("build.rs");
            if standard.is_file() {
                standard
            } else {
                let nested = pkg_dir.join("build").join("build.rs");
                if nested.is_file() {
                    nested
                } else {
                    return Ok(BuildScriptDirectives::default());
                }
            }
        }
    };
    if !build_rs.is_file() {
        return Ok(BuildScriptDirectives::default());
    }

    let bin_path = staging_dir.join("build_script_build");
    let out_dir = staging_dir.join("out");
    fs::create_dir_all(&out_dir)?;

    let (major, minor, patch, pre) = if let Ok(sem) = semver::Version::parse(&unit.version) {
        (
            sem.major.to_string(),
            sem.minor.to_string(),
            sem.patch.to_string(),
            sem.pre.as_str().to_string(),
        )
    } else {
        let mut ver_parts = unit.version.split('.');
        let maj = ver_parts.next().unwrap_or("0");
        let min = ver_parts.next().unwrap_or("0");
        let pat_with_pre = ver_parts.next().unwrap_or("0");
        let (pat, p) = pat_with_pre.split_once('-').unwrap_or((pat_with_pre, ""));
        (
            maj.to_string(),
            min.to_string(),
            pat.to_string(),
            p.to_string(),
        )
    };

    // Compile build.rs with standard Cargo compile-time environment
    let mut compile_cmd = Command::new("rustc");
    compile_cmd.current_dir(staging_dir);
    if let Ok(tc) = std::env::var("RUSTUP_TOOLCHAIN") {
        compile_cmd.env("RUSTUP_TOOLCHAIN", tc);
    }
    compile_cmd
        .arg("--crate-name")
        .arg("build_script_build")
        .arg(&build_rs)
        .arg("--crate-type")
        .arg("bin")
        .arg("--edition")
        .arg(unit.edition.as_deref().unwrap_or("2015"))
        .arg("-o")
        .arg(&bin_path)
        .env("CARGO_MANIFEST_DIR", pkg_dir)
        .env("CARGO", std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string()))
        .env("CARGO", std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string()))
        .env("CARGO_PKG_NAME", &unit.package_name)
        .env("CARGO_PKG_VERSION", &unit.version)
        .env("CARGO_PKG_VERSION_MAJOR", &major)
        .env("CARGO_PKG_VERSION_MINOR", &minor)
        .env("CARGO_PKG_VERSION_PATCH", &patch)
        .env("CARGO_PKG_VERSION_PRE", &pre);

    if let Some(ref links) = pkg_toml.package.as_ref().and_then(|p| p.links.as_ref()) {
        compile_cmd.env("CARGO_MANIFEST_LINKS", links);
    }

    if let Some(extra) = extra_env {
        for (k, v) in extra {
            compile_cmd.env(k, v);
        }
    }

    for feat in features {
        compile_cmd
            .arg("--cfg")
            .arg(format!("feature=\"{}\"", feat));
        compile_cmd.env(
            format!("CARGO_FEATURE_{}", feat.to_uppercase().replace('-', "_")),
            "1",
        );
    }

    for (crate_name, artifact_path) in extern_crates {
        compile_cmd.arg("--extern").arg(format!(
            "{}={}",
            crate_name.replace('-', "_"),
            artifact_path.display()
        ));
    }

    if let Some(artifacts) = cas_artifacts_dir {
        compile_cmd
            .arg("-L")
            .arg(format!("dependency={}", artifacts.display()));

        let mut build_dep_names: Vec<String> =
            pkg_toml.build_dependencies.keys().cloned().collect();
        for target_spec in pkg_toml.target.values() {
            for dep in target_spec.build_dependencies.keys() {
                build_dep_names.push(dep.clone());
            }
        }
        for dep in &build_dep_names {
            let clean = dep.replace('-', "_");
            if !extern_crates
                .iter()
                .any(|(c, _)| c.replace('-', "_") == clean)
            {
                if let Ok(entries) = fs::read_dir(artifacts) {
                    let prefix = format!("lib{}-", clean);
                    for entry in entries.flatten() {
                        let fname = entry.file_name();
                        let fstr = fname.to_string_lossy();
                        if fstr.starts_with(&prefix) && fstr.ends_with(".rlib") {
                            compile_cmd.arg("--extern").arg(format!(
                                "{}={}",
                                clean,
                                entry.path().display()
                            ));
                            break;
                        }
                    }
                }
            }
        }
    }

    let compile_status = compile_cmd.output().context("Failed to compile build.rs")?;

    if !compile_status.status.success() {
        let err = String::from_utf8_lossy(&compile_status.stderr);
        bail!(
            "Failed to compile build.rs for '{}': {}",
            unit.package_name,
            err
        );
    }

    let rustc_bin = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
    let target = std::env::var("TARGET").unwrap_or_else(|_| {
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            "aarch64-apple-darwin".to_string()
        }
        #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
        {
            "x86_64-apple-darwin".to_string()
        }
        #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
        {
            "x86_64-unknown-linux-gnu".to_string()
        }
        #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
        {
            "aarch64-unknown-linux-gnu".to_string()
        }
        #[cfg(not(any(
            all(target_os = "macos", target_arch = "aarch64"),
            all(target_os = "macos", target_arch = "x86_64"),
            all(target_os = "linux", target_arch = "x86_64"),
            all(target_os = "linux", target_arch = "aarch64"),
        )))]
        {
            "host".to_string()
        }
    });

    let mut run_cmd = Command::new(&bin_path);
    run_cmd
        .current_dir(pkg_dir)
        .env("CARGO_MANIFEST_DIR", pkg_dir)
        .env("CARGO_PKG_NAME", &unit.package_name)
        .env("CARGO_PKG_VERSION", &unit.version)
        .env("CARGO_PKG_VERSION_MAJOR", &major)
        .env("CARGO_PKG_VERSION_MINOR", &minor)
        .env("CARGO_PKG_VERSION_PATCH", &patch)
        .env("CARGO_PKG_VERSION_PRE", &pre)
        .env(
            "NUM_JOBS",
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4)
                .to_string(),
        )
        .env("OUT_DIR", &out_dir)
        .env("TARGET", &target)
        .env("HOST", &target)
        .env("RUSTC", &rustc_bin)
        .env("PROFILE", if release { "release" } else { "debug" })
        .env("OPT_LEVEL", if release { "3" } else { "0" })
        .env("DEBUG", if release { "false" } else { "true" });

    if let Some(ref links) = pkg_toml.package.as_ref().and_then(|p| p.links.as_ref()) {
        run_cmd.env("CARGO_MANIFEST_LINKS", links);
    }

    #[cfg(target_os = "macos")]
    {
        run_cmd.env("CARGO_CFG_TARGET_OS", "macos");
        run_cmd.env("CARGO_CFG_TARGET_FAMILY", "unix");
        run_cmd.env("CARGO_CFG_TARGET_VENDOR", "apple");
        run_cmd.env("CARGO_CFG_TARGET_ENV", "");
    }
    #[cfg(target_os = "linux")]
    {
        run_cmd.env("CARGO_CFG_TARGET_OS", "linux");
        run_cmd.env("CARGO_CFG_TARGET_FAMILY", "unix");
        run_cmd.env("CARGO_CFG_TARGET_VENDOR", "unknown");
        run_cmd.env("CARGO_CFG_TARGET_ENV", "gnu");
    }
    #[cfg(target_arch = "aarch64")]
    {
        run_cmd.env("CARGO_CFG_TARGET_ARCH", "aarch64");
        run_cmd.env("CARGO_CFG_TARGET_POINTER_WIDTH", "64");
        run_cmd.env("CARGO_CFG_TARGET_ENDIAN", "little");
    }
    #[cfg(target_arch = "x86_64")]
    {
        run_cmd.env("CARGO_CFG_TARGET_ARCH", "x86_64");
        run_cmd.env("CARGO_CFG_TARGET_POINTER_WIDTH", "64");
        run_cmd.env("CARGO_CFG_TARGET_ENDIAN", "little");
    }
    run_cmd.env(
        "NUM_JOBS",
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(8)
            .to_string(),
    );
    if let Ok(tc) = std::env::var("RUSTUP_TOOLCHAIN") {
        run_cmd.env("RUSTUP_TOOLCHAIN", tc);
    }

    for feat in features {
        let env_key = format!("CARGO_FEATURE_{}", feat.to_uppercase().replace('-', "_"));
        run_cmd.env(env_key, "1");
    }

    if let Some(extra) = extra_env {
        for (k, v) in extra {
            run_cmd.env(k, v);
        }
    }

    let run_status = run_cmd.output().context("Failed to execute build.rs")?;

    if !run_status.status.success() {
        let err = String::from_utf8_lossy(&run_status.stderr);
        bail!("build.rs failed for '{}': {}", unit.package_name, err);
    }

    let links = pkg_toml.package.as_ref().and_then(|p| p.links.clone());
    let stdout_str = String::from_utf8_lossy(&run_status.stdout);
    let mut directives = BuildScriptDirectives::parse_stdout(&stdout_str);
    directives.out_dir = Some(out_dir);
    directives.links = links;

    Ok(directives)
}
