/*
 * Mitts Build — package and registry commands
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anstream::eprintln;
use anyhow::{Context, Result};
use clap::Args;
use std::path::{Path, PathBuf};

use super::{package_archive, package_directory, package_lock, registry_metadata, registry_upload};

#[derive(Args, Debug)]
pub struct PackageArgs {
    /// Path to the package manifest (default: the nearest Cargo.toml here or in a parent)
    #[arg(long)]
    pub manifest_path: Option<PathBuf>,
    /// Output directory for the crate archive
    #[arg(long)]
    pub target_dir: Option<PathBuf>,
    /// Shared Mitts cache for verification builds
    #[arg(long)]
    pub cache_dir: Option<PathBuf>,
    /// Require an existing compatible lockfile
    #[arg(long)]
    pub locked: bool,
    /// Resolve using local data only
    #[arg(long)]
    pub offline: bool,
    /// Combine locked and offline resolution
    #[arg(long)]
    pub frozen: bool,
    /// Skip building the extracted package
    #[arg(long)]
    pub no_verify: bool,
}

#[derive(Args, Debug)]
pub struct PublishArgs {
    #[command(flatten)]
    pub package: PackageArgs,
    /// Prepare the package without uploading it
    #[arg(long)]
    pub dry_run: bool,
    /// Publish a previously prepared crate archive
    #[arg(long)]
    pub archive: Option<PathBuf>,
}

fn prepared_archive(
    path: &Path,
    manifest_path: &Path,
    metadata: &serde_json::Value,
) -> Result<Vec<u8>> {
    use anyhow::bail;
    use std::io::Read;
    let name = metadata["name"].as_str().context("invalid package name")?;
    let version = metadata["vers"]
        .as_str()
        .context("invalid package version")?;
    let prefix = format!("{name}-{version}");
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(100 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 100 * 1024 * 1024 {
        bail!("crate archive is too large");
    }
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(bytes.as_slice()));
    let mut seen = std::collections::HashSet::new();
    let mut found_manifest = false;
    let mut found_normalized = false;
    let mut unpacked_size = 0u64;
    for entry in archive.entries()? {
        let entry = entry?;
        if !entry.header().entry_type().is_file() && !entry.header().entry_type().is_dir() {
            bail!("unsupported archive entry");
        }
        unpacked_size = unpacked_size
            .checked_add(entry.size())
            .context("archive size overflow")?;
        if unpacked_size > 512 * 1024 * 1024 {
            bail!("unpacked crate is too large");
        }
        let path = entry.path()?.into_owned();
        let mut parts = path.components();
        if parts.next() != Some(std::path::Component::Normal(std::ffi::OsStr::new(&prefix)))
            || parts.any(|part| !matches!(part, std::path::Component::Normal(_)))
            || !seen.insert(path.clone())
        {
            bail!("invalid or duplicate archive path");
        }
        if path == Path::new(&prefix).join("Cargo.toml") {
            let mut raw = String::new();
            entry.take(1024 * 1024 + 1).read_to_string(&mut raw)?;
            if raw.len() > 1024 * 1024 {
                bail!("archive manifest is too large");
            }
            let normalized: toml::Value = toml::from_str(&raw)?;
            let mut expected: toml::Value =
                toml::from_str(&std::fs::read_to_string(manifest_path)?)?;
            let table = expected
                .as_table_mut()
                .context("manifest must be a table")?;
            for section in ["workspace", "patch", "replace"] {
                table.remove(section);
            }
            if normalized != expected {
                bail!("normalized archive manifest differs from source");
            }
            found_normalized = true;
        } else if path == Path::new(&prefix).join("Cargo.toml.orig") {
            let mut raw = String::new();
            entry.take(1024 * 1024 + 1).read_to_string(&mut raw)?;
            if raw.len() > 1024 * 1024 {
                bail!("archive manifest is too large");
            }
            let manifest: toml::Value = toml::from_str(&raw)?;
            let source: toml::Value = toml::from_str(&std::fs::read_to_string(manifest_path)?)?;
            if manifest != source {
                bail!("archive manifest differs from source manifest");
            }
            let package = manifest
                .get("package")
                .and_then(toml::Value::as_table)
                .context("missing package")?;
            if package.get("name").and_then(toml::Value::as_str) != Some(name)
                || package.get("version").and_then(toml::Value::as_str) != Some(version)
            {
                bail!("archive manifest does not match package");
            }
            found_manifest = true;
        }
    }
    if !found_manifest || !found_normalized {
        bail!("archive is missing a required manifest");
    }
    Ok(bytes)
}

impl PackageArgs {
    /// The package manifest: `--manifest-path`, else the nearest `Cargo.toml` up the tree.
    fn manifest(&self) -> Result<PathBuf> {
        let path = super::manifest_discovery::discover_manifest(&self.manifest_path)
            .unwrap_or_else(|| PathBuf::from("Cargo.toml"));
        std::fs::canonicalize(&path)
            .with_context(|| format!("cannot open manifest {}", path.display()))
    }
}

/// The `Cargo.lock` that governs the package: its workspace root's, like Cargo.
fn lock_path_for(manifest: &Path) -> PathBuf {
    use crate::adapter::rust::manifest::CargoToml;
    CargoToml::from_file(manifest)
        .ok()
        .and_then(|toml| {
            CargoToml::find_workspace_root(manifest, &toml)
                .ok()
                .flatten()
        })
        .map_or_else(|| manifest.to_path_buf(), |(root, _)| root)
        .with_file_name("Cargo.lock")
}

fn prepare(args: &PackageArgs) -> Result<(serde_json::Value, Vec<u8>)> {
    let manifest = args.manifest()?;
    let root = manifest.parent().context("manifest has no parent")?;
    let metadata = registry_metadata::from_manifest(&manifest)?;
    let name = metadata["name"].as_str().context("invalid package name")?;
    let version = metadata["vers"]
        .as_str()
        .context("invalid package version")?;
    let lock_path = lock_path_for(&manifest);
    let lock = if lock_path.is_file() {
        let lock = crate::adapter::rust::lockfile::CargoLock::from_file(&lock_path)?;
        if !package_lock::compatible(&lock, &manifest, name, version)? {
            if args.locked || args.frozen {
                anyhow::bail!("Cargo.lock does not match the package manifest");
            }
            crate::adapter::rust::resolver::NativeResolver::for_cache_dir(
                args.cache_dir.as_deref(),
            )?
            .resolve_manifest(&manifest, args.offline || args.frozen)?
            .to_string_formatted()
            .into_bytes()
        } else if lock_path.parent() == Some(root) {
            std::fs::read(&lock_path)?
        } else {
            // A workspace member's archive carries a lock of its own graph, not the
            // workspace's (which lists sibling path packages the archive does not contain).
            crate::adapter::rust::resolver::NativeResolver::for_cache_dir(
                args.cache_dir.as_deref(),
            )?
            .resolve_manifest(&manifest, args.offline || args.frozen)?
            .to_string_formatted()
            .into_bytes()
        }
    } else if args.locked || args.frozen {
        anyhow::bail!("Cargo.lock is missing and locked resolution was requested");
    } else {
        crate::adapter::rust::resolver::NativeResolver::for_cache_dir(args.cache_dir.as_deref())?
            .resolve_manifest(&manifest, args.offline || args.frozen)?
            .to_string_formatted()
            .into_bytes()
    };
    let in_git = std::process::Command::new("git")
        .current_dir(root)
        .args(["rev-parse", "--is-inside-work-tree"])
        .output()
        .is_ok_and(|output| output.status.success() && output.stdout == b"true\n");
    if !in_git && root.ancestors().any(|dir| dir.join(".git").exists()) {
        anyhow::bail!("Git checkout detected but its repository could not be inspected");
    }
    let archive = if in_git {
        package_archive::create_with_lock(root, name, version, Some(&lock))?
    } else {
        package_directory::create(root, name, version, &lock)?
    };
    if registry_metadata::from_manifest(&manifest)? != metadata {
        anyhow::bail!("package metadata changed during archive creation");
    }
    let target = crate::adapter::RustAdapter::resolve_target_dir_without_tags(
        &manifest,
        &crate::adapter::RustBuildOptions {
            target_dir: args.target_dir.clone(),
            ..Default::default()
        },
    )?;
    let directory = target.join("package");
    std::fs::create_dir_all(&directory)?;
    let output = directory.join(format!("{name}-{version}.crate"));
    std::fs::write(&output, &archive)?;
    eprintln!("Packaged {}", output.display());
    Ok((metadata, archive))
}

fn ensure_publish_allowed(path: &Path) -> Result<()> {
    let manifest: toml::Value = toml::from_str(&std::fs::read_to_string(path)?)?;
    let publish = manifest.get("package").and_then(|p| p.get("publish"));
    match publish {
        None | Some(toml::Value::Boolean(true)) => Ok(()),
        Some(toml::Value::Array(registries))
            if registries.iter().any(|v| v.as_str() == Some("crates-io")) =>
        {
            Ok(())
        }
        _ => anyhow::bail!("package manifest does not allow publication to crates.io"),
    }
}

fn verify_archive(
    archive: &[u8],
    metadata: &serde_json::Value,
    cache_dir: Option<&Path>,
    offline: bool,
) -> Result<()> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = make_directories_writable(&self.0);
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let temp = std::env::temp_dir().join(format!(
        "mitts-package-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir(&temp)?;
    let _cleanup = Cleanup(temp.clone());
    let name = metadata["name"].as_str().context("invalid package name")?;
    let version = metadata["vers"]
        .as_str()
        .context("invalid package version")?;
    let mut input = tar::Archive::new(flate2::read::GzDecoder::new(archive));
    input.unpack(&temp)?;
    let root = temp.join(format!("{name}-{version}"));
    if !root.join("Cargo.lock").is_file() {
        anyhow::bail!("package verification currently requires Cargo.lock");
    }
    reject_symlinks(&root)?;
    let before = crate::core::hash::Sha256::digest_path_recursive(&root)?;
    let mut command = std::process::Command::new(std::env::current_exe()?);
    command
        .env_remove("CARGO_REGISTRY_TOKEN")
        .arg("build")
        .arg("--locked")
        .arg("--manifest-path")
        .arg(root.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(temp.join("target"))
        .arg("--cache-dir")
        .arg(
            cache_dir
                .map(Path::to_path_buf)
                .unwrap_or_else(|| temp.join("cache")),
        );
    if offline {
        command.arg("--offline");
    }
    let status = command.status()?;
    if !status.success() {
        anyhow::bail!("building the extracted package failed");
    }
    reject_symlinks(&root)?;
    if crate::core::hash::Sha256::digest_path_recursive(&root)? != before {
        anyhow::bail!("building the extracted package modified its source files");
    }
    Ok(())
}

fn reject_symlinks(path: &Path) -> Result<()> {
    let meta = std::fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        anyhow::bail!("package verification found a symbolic link");
    }
    if meta.is_dir() {
        for entry in std::fs::read_dir(path)? {
            reject_symlinks(&entry?.path())?;
        }
    }
    Ok(())
}

fn make_directories_writable(path: &Path) -> std::io::Result<()> {
    let meta = std::fs::symlink_metadata(path)?;
    if !meta.file_type().is_dir() {
        return Ok(());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = meta.permissions().mode();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode | 0o700))?;
    }
    for entry in std::fs::read_dir(path)? {
        make_directories_writable(&entry?.path())?;
    }
    Ok(())
}

pub fn handle_package(args: &PackageArgs) -> i32 {
    let result = (|| -> Result<()> {
        let (metadata, archive) = prepare(args)?;
        if !args.no_verify {
            verify_archive(
                &archive,
                &metadata,
                args.cache_dir.as_deref(),
                args.offline || args.frozen,
            )?;
        }
        Ok(())
    })();
    match result {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("package failed: {error:#}");
            1
        }
    }
}

pub fn handle_publish(args: &PublishArgs) -> i32 {
    let result = (|| -> Result<()> {
        let manifest = args.package.manifest()?;
        ensure_publish_allowed(&manifest)?;
        if !args.dry_run && (args.package.offline || args.package.frozen) {
            anyhow::bail!("publication requires network access");
        }
        let (metadata, archive) = if let Some(path) = &args.archive {
            if args.package.locked || args.package.frozen {
                let lock_path = lock_path_for(&manifest);
                let lock = crate::adapter::rust::lockfile::CargoLock::from_file(&lock_path)?;
                let metadata = registry_metadata::from_manifest(&manifest)?;
                let name = metadata["name"].as_str().context("invalid package name")?;
                let version = metadata["vers"]
                    .as_str()
                    .context("invalid package version")?;
                if !package_lock::compatible(&lock, &manifest, name, version)? {
                    anyhow::bail!("Cargo.lock does not match the package manifest");
                }
            }
            let metadata = registry_metadata::from_manifest(&manifest)?;
            let archive = prepared_archive(path, &manifest, &metadata)?;
            (metadata, archive)
        } else {
            prepare(&args.package)?
        };
        if !args.package.no_verify {
            verify_archive(
                &archive,
                &metadata,
                args.package.cache_dir.as_deref(),
                args.package.offline || args.package.frozen,
            )?;
        }
        if !args.dry_run {
            let token = std::env::var("CARGO_REGISTRY_TOKEN")
                .context("CARGO_REGISTRY_TOKEN is required")?;
            registry_upload::upload(&metadata, &archive, &token)?;
        }
        Ok(())
    })();
    match result {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("publish failed: {error:#}");
            1
        }
    }
}

#[cfg(test)]
#[path = "package_tests.rs"]
mod tests;
