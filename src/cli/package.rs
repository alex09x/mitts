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
    /// Package(s) to assemble
    #[arg(short = 'p', long = "package", value_name = "SPEC", action = clap::ArgAction::Append)]
    pub packages: Vec<String>,
    /// Assemble all packages in the workspace
    #[arg(long, visible_alias = "all")]
    pub workspace: bool,
    /// Don't assemble specified packages
    #[arg(long, value_name = "SPEC", action = clap::ArgAction::Append)]
    pub exclude: Vec<String>,
    /// Print files included in a package without making one
    #[arg(short = 'l', long)]
    pub list: bool,
    /// Allow dirty working directories to be packaged
    #[arg(long)]
    pub allow_dirty: bool,
    /// Ignore warnings about a lack of human-usable metadata
    #[arg(long)]
    pub no_metadata: bool,
    /// Don't include the lock file when packaging (unstable in Cargo)
    #[arg(long)]
    pub exclude_lockfile: bool,
    /// Space or comma separated list of features to activate (verification build)
    #[arg(short = 'F', long, value_delimiter = ',', num_args = 1..)]
    pub features: Vec<String>,
    /// Activate all available features
    #[arg(long)]
    pub all_features: bool,
    /// Do not activate the `default` feature
    #[arg(long)]
    pub no_default_features: bool,
    /// Build for the target triple (verification build)
    #[arg(long)]
    pub target: Option<String>,
    /// Number of parallel jobs
    #[arg(short = 'j', long, value_name = "N")]
    pub jobs: Option<u64>,
    /// Do not abort the build as soon as there is an error
    #[arg(long)]
    pub keep_going: bool,
    /// Registry to package for / publish to
    #[arg(long)]
    pub registry: Option<String>,
    /// Registry index URL to package for / publish to
    #[arg(long)]
    pub index: Option<String>,
    /// Output representation (unstable in Cargo)
    #[arg(long, value_name = "FMT")]
    pub message_format: Option<String>,
    /// Do not print Mitts log messages
    #[arg(short, long)]
    pub quiet: bool,
    /// Use verbose output
    #[arg(short, long, action = clap::ArgAction::Count)]
    pub verbose: u8,
}

#[derive(Args, Debug)]
pub struct PublishArgs {
    #[command(flatten)]
    pub package: PackageArgs,
    /// Prepare the package without uploading it
    #[arg(short = 'n', long)]
    pub dry_run: bool,
    /// Publish a previously prepared crate archive
    #[arg(long)]
    pub archive: Option<PathBuf>,
    /// Token to use when uploading
    #[arg(long)]
    pub token: Option<String>,
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

/// The manifests of the packages `-p` / `--workspace` / `--exclude` select, else the one
/// package `--manifest-path` (or the current directory) names.
fn selected_manifests(args: &PackageArgs) -> Result<Vec<PathBuf>> {
    if args.exclude_lockfile {
        anyhow::bail!(
            "the `--exclude-lockfile` flag is unstable in Cargo and not supported by Mitts"
        );
    }
    if args.message_format.is_some() {
        anyhow::bail!(
            "`--message-format` for packaging is unstable in Cargo and not supported by Mitts"
        );
    }
    if args.packages.is_empty() && !args.workspace {
        return Ok(vec![args.manifest()?]);
    }
    let start = args.manifest()?;
    let root = super::project_info::workspace_root(&start)?;
    let members = super::project_info::local_package_dirs(&root)?;
    // `-p name@version`: the version must be the member's.
    for spec in &args.packages {
        let (name, version) = super::project_info::parse_spec(spec);
        if let (Some(wanted), Some((_, dir))) =
            (version, members.iter().find(|(n, _)| n == name))
        {
            let have = crate::adapter::rust::manifest::CargoToml::from_file(&dir.join("Cargo.toml"))?
                .package
                .and_then(|p| p.version)
                .unwrap_or_default();
            let matches = have == wanted || have.starts_with(&format!("{wanted}."));
            if !matches {
                anyhow::bail!("package ID specification `{spec}` did not match any packages");
            }
        }
    }
    let mut out = Vec::new();
    for (name, dir) in &members {
        let named = args
            .packages
            .iter()
            .any(|spec| super::project_info::parse_spec(spec).0 == name);
        let excluded = args
            .exclude
            .iter()
            .any(|spec| super::project_info::parse_spec(spec).0 == name);
        if (args.workspace || named) && !excluded {
            out.push(std::fs::canonicalize(dir.join("Cargo.toml"))?);
        }
    }
    for spec in &args.packages {
        let (name, _) = super::project_info::parse_spec(spec);
        if !members.iter().any(|(n, _)| n == name) {
            anyhow::bail!("package ID specification `{spec}` did not match any packages");
        }
    }
    Ok(out)
}

/// Cargo's refusal to package uncommitted changes: modified or untracked files under the
/// package directory, unless `--allow-dirty`.
fn ensure_clean(root: &Path, allow_dirty: bool) -> Result<()> {
    if allow_dirty {
        return Ok(());
    }
    let out = std::process::Command::new("git")
        .current_dir(root)
        .args(["status", "--porcelain", "--untracked-files=all", "--", "."])
        .output()?;
    if !out.status.success() {
        return Ok(());
    }
    let prefix = std::process::Command::new("git")
        .current_dir(root)
        .args(["rev-parse", "--show-prefix"])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    let dirty: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| l.len() > 3)
        .map(|l| l[3..].trim().trim_matches('"').to_string())
        .map(|p| p.strip_prefix(&prefix).map(str::to_string).unwrap_or(p))
        .filter(|p| p != "Cargo.lock")
        .collect();
    if dirty.is_empty() {
        return Ok(());
    }
    anyhow::bail!(
        "{} files in the working directory contain changes that were not yet committed into git:\n\n{}\n\nto proceed despite this and include the uncommitted changes, pass the `--allow-dirty` flag",
        dirty.len(),
        dirty.join("\n")
    )
}

/// The paths inside a crate archive, without the `name-version/` prefix.
fn archive_files(archive: &[u8]) -> Result<Vec<String>> {
    let mut input = tar::Archive::new(flate2::read::GzDecoder::new(archive));
    let mut files = Vec::new();
    for entry in input.entries()? {
        let entry = entry?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let path = entry.path()?.into_owned();
        let rel: PathBuf = path.components().skip(1).collect();
        files.push(rel.to_string_lossy().replace('\\', "/"));
    }
    files.sort();
    Ok(files)
}

fn prepare(args: &PackageArgs, manifest: &Path) -> Result<(serde_json::Value, Vec<u8>)> {
    let manifest = manifest.to_path_buf();
    let root = manifest.parent().context("manifest has no parent")?;
    let metadata = registry_metadata::from_manifest(&manifest)?;
    let name = metadata["name"].as_str().context("invalid package name")?;
    let version = metadata["vers"]
        .as_str()
        .context("invalid package version")?;
    let lock_path = lock_path_for(&manifest);
    // Re-resolving from an existing lock keeps its versions where they fit and its format
    // version, as Cargo does.
    let re_resolve = |lock: &crate::adapter::rust::lockfile::CargoLock| -> Result<Vec<u8>> {
        let mut fresh = crate::adapter::rust::resolver::NativeResolver::for_cache_dir(
            args.cache_dir.as_deref(),
        )?
        .prefer_locked(lock)
        .resolve_manifest(&manifest, args.offline || args.frozen)?;
        fresh.version = fresh.version.max(lock.version);
        Ok(fresh.to_string_formatted().into_bytes())
    };
    let lock = if lock_path.is_file() {
        let lock = crate::adapter::rust::lockfile::CargoLock::from_file(&lock_path)?;
        if !package_lock::compatible(&lock, &manifest, name, version)? {
            if args.locked || args.frozen {
                anyhow::bail!("Cargo.lock does not match the package manifest");
            }
            re_resolve(&lock)?
        } else if lock_path.parent() == Some(root) {
            std::fs::read(&lock_path)?
        } else {
            // A workspace member's archive carries a lock of its own graph, not the
            // workspace's (which lists sibling path packages the archive does not contain).
            re_resolve(&lock)?
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
    if in_git && !args.list {
        ensure_clean(root, args.allow_dirty)?;
    }
    // `--allow-dirty` packages the working tree as it is, uncommitted changes included.
    let archive = if in_git && !args.allow_dirty && !args.list {
        package_archive::create_with_lock(root, name, version, Some(&lock))?
    } else if in_git {
        // The working tree as it is, but never what Git ignores (secrets, build output).
        let out = std::process::Command::new("git")
            .current_dir(root)
            .args(["ls-files", "--others", "--ignored", "--exclude-standard", "-z", "--", "."])
            .output()?;
        if !out.status.success() {
            anyhow::bail!("git ls-files failed in {}", root.display());
        }
        let ignored: std::collections::HashSet<String> = String::from_utf8_lossy(&out.stdout)
            .split('\0')
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
        package_directory::create_skipping(root, name, version, &lock, Some(&ignored))?
    } else {
        package_directory::create(root, name, version, &lock)?
    };
    if registry_metadata::from_manifest(&manifest)? != metadata {
        anyhow::bail!("package metadata changed during archive creation");
    }
    if args.list {
        for file in archive_files(&archive)? {
            println!("{file}");
        }
        return Ok((metadata, Vec::new()));
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
    if !args.quiet {
        eprintln!("Packaged {}", output.display());
    }
    Ok((metadata, archive))
}

/// `package.publish` against the destination; `None` is an index no registry name is
/// configured for, which only an unrestricted package may go to.
fn ensure_publish_allowed(path: &Path, registry: Option<&str>) -> Result<()> {
    let manifest: toml::Value = toml::from_str(&std::fs::read_to_string(path)?)?;
    let publish = manifest.get("package").and_then(|p| p.get("publish"));
    let shown = registry.unwrap_or("an unnamed index");
    match publish {
        None | Some(toml::Value::Boolean(true)) => Ok(()),
        Some(toml::Value::Array(registries))
            if registry.is_some_and(|r| registries.iter().any(|v| v.as_str() == Some(r))) =>
        {
            Ok(())
        }
        _ => anyhow::bail!("package manifest does not allow publication to {shown}"),
    }
}

fn verify_archive(
    archive: &[u8],
    metadata: &serde_json::Value,
    cache_dir: Option<&Path>,
    offline: bool,
    args: &PackageArgs,
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
        .args(crate::adapter::rust::cargo_config_cli::child_args())
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
    if !args.features.is_empty() {
        command.args(["--features", &args.features.join(",")]);
    }
    for (flag, on) in [
        ("--all-features", args.all_features),
        ("--no-default-features", args.no_default_features),
        ("--keep-going", args.keep_going),
        ("--quiet", args.quiet),
    ] {
        if on {
            command.arg(flag);
        }
    }
    if let Some(target) = &args.target {
        command.args(["--target", target]);
    }
    if let Some(jobs) = args.jobs {
        command.args(["--jobs", &jobs.to_string()]);
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
        for manifest in selected_manifests(args)? {
            let (metadata, archive) = prepare(args, &manifest)?;
            if !args.no_verify && !args.list {
                verify_archive(
                    &archive,
                    &metadata,
                    args.cache_dir.as_deref(),
                    args.offline || args.frozen,
                    args,
                )?;
            }
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
        if args.package.list {
            anyhow::bail!("`--list` does not publish anything; use `mitts package --list`");
        }
        let manifests = if args.archive.is_some() {
            vec![args.package.manifest()?]
        } else {
            selected_manifests(&args.package)?
        };
        for manifest in manifests {
            publish_one(args, &manifest)?;
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

fn publish_one(args: &PublishArgs, manifest: &Path) -> Result<()> {
    let manifest = manifest.to_path_buf();
    {
        // The registry the upload really goes to decides what `package.publish` allows: an
        // `--index` is checked under the name it is configured as, an unknown one as none.
        let project_dir = manifest.parent().unwrap_or(Path::new("."));
        let destination = match (&args.package.index, &args.package.registry) {
            (Some(index), None) => {
                crate::adapter::rust::registry_config::RegistryConfig::load(project_dir)?
                    .registry_name_for_index(index)
            }
            (_, registry) => Some(registry.clone().unwrap_or_else(|| "crates-io".into())),
        };
        ensure_publish_allowed(&manifest, destination.as_deref())?;
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
            prepare(&args.package, &manifest)?
        };
        if !args.package.no_verify {
            verify_archive(
                &archive,
                &metadata,
                args.package.cache_dir.as_deref(),
                args.package.offline || args.package.frozen,
                &args.package,
            )?;
        }
        if args.dry_run {
            if !args.package.quiet {
                eprintln!("warning: aborting upload due to dry run");
            }
            return Ok(());
        }
        let project_dir = manifest.parent().unwrap_or(Path::new("."));
        let crates_io = args
            .package
            .registry
            .as_deref()
            .is_none_or(|r| r == "crates-io")
            && args.package.index.is_none();
        if crates_io && std::env::var_os("MITTS_CRATES_IO_API").is_none() {
            let token = match &args.token {
                Some(t) => t.clone(),
                None => crate::adapter::rust::registry_config::RegistryConfig::load(project_dir)?
                    .token_for_registry(None)?
                    .context(
                        "no token found, please run `mitts login` or set CARGO_REGISTRY_TOKEN",
                    )?,
            };
            registry_upload::upload(&metadata, &archive, &token)?;
        } else {
            let mut api = super::registry_api::RegistryApi::open(
                project_dir,
                args.package.registry.as_deref(),
                args.package.index.as_deref(),
            )?;
            if let Some(t) = &args.token {
                api.token_override = Some(t.clone());
            }
            let token = api.token()?;
            registry_upload::upload_to_api(&api.url("/crates/new"), &metadata, &archive, &token)?;
        }
        if !args.package.quiet {
            eprintln!(
                "{:>12} {} v{} to registry `{}`",
                "Uploaded",
                metadata["name"].as_str().unwrap_or_default(),
                metadata["vers"].as_str().unwrap_or_default(),
                args.package.registry.as_deref().unwrap_or("crates-io")
            );
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "package_tests.rs"]
mod tests;
