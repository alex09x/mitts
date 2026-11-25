/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! `mitts locate-project`, `mitts pkgid` and `mitts version`, printed as Cargo prints them.

use crate::adapter::rust::lockfile::{CargoLock, CargoLockPackage};
use crate::adapter::rust::manifest::CargoToml;
use anyhow::{bail, Context, Result};
use clap::Args;
use std::path::{Path, PathBuf};

#[derive(Args, Debug, Clone, Default)]
pub struct LocateProjectArgs {
    /// Path to Cargo.toml
    #[arg(long)]
    pub manifest_path: Option<PathBuf>,

    /// Locate the Cargo.toml of the workspace root
    #[arg(long)]
    pub workspace: bool,

    /// Output representation: json or plain
    #[arg(long, default_value = "json", value_parser = ["json", "plain"])]
    pub message_format: String,

    /// Do not print Mitts log messages
    #[arg(short, long)]
    pub quiet: bool,

    /// Use verbose output
    #[arg(short, long, action = clap::ArgAction::Count)]
    pub verbose: u8,
}

#[derive(Args, Debug, Clone, Default)]
pub struct PkgidArgs {
    /// Package to print the ID of (a name, `name@version` or `name:version`)
    #[arg(value_name = "SPEC")]
    pub spec: Option<String>,

    /// Argument to get the package ID specifier for
    #[arg(short = 'p', long = "package", value_name = "SPEC")]
    pub package: Option<String>,

    /// Path to Cargo.toml
    #[arg(long)]
    pub manifest_path: Option<PathBuf>,

    /// Directory for the shared artifact and action cache
    #[arg(long)]
    pub cache_dir: Option<PathBuf>,

    /// Require Cargo.lock is up to date
    #[arg(long)]
    pub locked: bool,

    /// Run without accessing the network
    #[arg(long)]
    pub offline: bool,

    /// Equivalent to specifying both --locked and --offline
    #[arg(long)]
    pub frozen: bool,

    /// Do not print Mitts log messages
    #[arg(short, long)]
    pub quiet: bool,

    /// Use verbose output
    #[arg(short, long, action = clap::ArgAction::Count)]
    pub verbose: u8,
}

#[derive(Args, Debug, Clone, Default)]
pub struct VersionArgs {
    /// Use verbose output
    #[arg(short, long, action = clap::ArgAction::Count)]
    pub verbose: u8,
}

fn report(result: Result<()>) -> i32 {
    match result {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("error: {error:#}");
            101
        }
    }
}

/// The manifest a command starts from, or Cargo's error when there is none.
pub fn starting_manifest(explicit: &Option<PathBuf>) -> Result<PathBuf> {
    let found = super::manifest_discovery::discover_manifest(explicit);
    match found {
        Some(path) if path.is_file() => Ok(path),
        Some(path) => bail!("manifest path `{}` does not exist", path.display()),
        None => {
            let cwd = std::env::current_dir().unwrap_or_default();
            bail!(
                "could not find `Cargo.toml` in `{}` or any parent directory",
                cwd.display()
            )
        }
    }
}

/// The root manifest of the workspace `manifest` belongs to (itself when it is none).
pub fn workspace_root(manifest: &Path) -> Result<PathBuf> {
    let toml = CargoToml::from_file(manifest)
        .with_context(|| format!("failed to parse manifest at `{}`", manifest.display()))?;
    Ok(CargoToml::find_workspace_root(manifest, &toml)?
        .map(|(root, _)| root)
        .unwrap_or_else(|| manifest.to_path_buf()))
}

pub fn handle_locate_project(args: &LocateProjectArgs) -> i32 {
    report((|| {
        let mut path = starting_manifest(&args.manifest_path)?;
        if args.workspace {
            path = workspace_root(&path)?;
        }
        if args.message_format == "plain" {
            println!("{}", path.display());
        } else {
            println!("{}", serde_json::json!({ "root": path.to_string_lossy() }));
        }
        Ok(())
    })())
}

/// The `path+file://` URL of a local directory as Cargo writes it: forward slashes, and a
/// Windows drive path behind a third slash (`path+file:///C:/w/app`), without the `\\?\`
/// prefix `canonicalize` adds there.
pub fn path_source_url(dir: &Path) -> String {
    let text = dir.to_string_lossy();
    let text = text
        .strip_prefix(r"\\?\")
        .unwrap_or(&text)
        .replace('\\', "/");
    if text.starts_with('/') {
        format!("path+file://{text}")
    } else {
        format!("path+file:///{text}")
    }
}

/// Cargo's package ID: `<source>#<name>@<version>`, with `path+file://` for local
/// packages and the name dropped when it is the directory's last component.
pub fn package_id(pkg: &CargoLockPackage, local_dir: Option<&Path>) -> String {
    match (&pkg.source, local_dir) {
        (None, Some(dir)) => {
            let url = path_source_url(dir);
            let last = dir.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if last == pkg.name {
                format!("{url}#{}", pkg.version)
            } else {
                format!("{url}#{}@{}", pkg.name, pkg.version)
            }
        }
        (Some(source), _) => {
            // A git source's `#commit` is the lock's pin, not part of the package ID.
            let base = source.split('#').next().unwrap_or(source);
            format!("{base}#{}@{}", pkg.name, pkg.version)
        }
        (None, None) => format!("{}@{}", pkg.name, pkg.version),
    }
}

/// Splits `name`, `name@version` or `name:version`.
pub fn parse_spec(spec: &str) -> (&str, Option<&str>) {
    let spec = spec.rsplit('#').next().unwrap_or(spec);
    match spec.split_once('@').or_else(|| spec.split_once(':')) {
        Some((name, version)) => (name, Some(version)),
        None => (spec, None),
    }
}

pub fn handle_pkgid(args: &PkgidArgs) -> i32 {
    report(run_pkgid(args))
}

fn run_pkgid(args: &PkgidArgs) -> Result<()> {
    let manifest = starting_manifest(&args.manifest_path)?;
    let root = workspace_root(&manifest)?;
    let lock = CargoLock::resolve_or_sync_with_mode(
        &root,
        args.offline || args.frozen,
        args.locked || args.frozen,
        args.cache_dir.as_deref(),
    )?;
    let local_dirs = local_package_dirs(&root)?;
    let spec = args.spec.clone().or_else(|| args.package.clone());
    let pkg = match spec {
        Some(spec) => select(&lock, &spec)?,
        None => {
            let toml = CargoToml::from_file(&manifest)?;
            let Some(package) = toml.package else {
                bail!(
                    "manifest path `{}` is a virtual manifest, but this command requires running \
                     against an actual package in this workspace",
                    manifest.display()
                );
            };
            lock.packages
                .iter()
                .find(|p| p.name == package.name && p.source.is_none())
                .context("the package is missing from Cargo.lock")?
                .clone()
        }
    };
    let dir = local_dirs
        .iter()
        .find(|(name, _)| *name == pkg.name)
        .map(|(_, dir)| dir.as_path());
    println!("{}", package_id(&pkg, dir));
    Ok(())
}

fn select(lock: &CargoLock, spec: &str) -> Result<CargoLockPackage> {
    let (name, version) = parse_spec(spec);
    let matches: Vec<&CargoLockPackage> = lock
        .packages
        .iter()
        .filter(|p| p.name == name && version.is_none_or(|v| version_matches(&p.version, v)))
        .collect();
    match matches.as_slice() {
        [one] => Ok((*one).clone()),
        [] => bail!("package ID specification `{spec}` did not match any packages"),
        many => {
            let mut text = format!(
                "There are multiple `{name}` packages in your project, and the specification \
                 `{spec}` is ambiguous.\nPlease re-run this command with one of the following \
                 specifications:"
            );
            for p in many {
                text.push_str(&format!("\n  {}@{}", p.name, p.version));
            }
            bail!(text)
        }
    }
}

/// A spec version may leave out the minor or patch number (`foo@1`), as in Cargo.
fn version_matches(version: &str, wanted: &str) -> bool {
    let have: Vec<&str> = version.split(['.', '-', '+']).collect();
    wanted
        .split('.')
        .enumerate()
        .all(|(i, part)| have.get(i) == Some(&part))
        || version == wanted
}

/// (package name, directory) for the workspace's local packages.
pub fn local_package_dirs(root: &Path) -> Result<Vec<(String, PathBuf)>> {
    let toml = CargoToml::from_file(root)?;
    let dir = root.parent().unwrap_or(Path::new(".")).to_path_buf();
    let mut out = Vec::new();
    if let Some(p) = &toml.package {
        out.push((p.name.clone(), dir.clone()));
    }
    if toml.workspace.is_some() {
        for (name, member_dir) in toml.discover_workspace_members(&dir)? {
            if !out.iter().any(|(n, _)| *n == name) {
                out.push((name, member_dir));
            }
        }
    }
    Ok(out)
}

pub fn handle_version(args: &VersionArgs) -> i32 {
    println!("mitts {}", super::version_string());
    if args.verbose > 0 {
        println!("release: {}", env!("CARGO_PKG_VERSION"));
        println!("commit-hash: {}", env!("MITTS_GIT_SHA"));
        println!("host: {}-{}", std::env::consts::ARCH, std::env::consts::OS);
        println!("os: {}", std::env::consts::OS);
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pkg(name: &str, version: &str, source: Option<&str>) -> CargoLockPackage {
        CargoLockPackage {
            name: name.into(),
            version: version.into(),
            source: source.map(Into::into),
            checksum: None,
            dependencies: Vec::new(),
        }
    }

    #[test]
    fn ids_are_written_like_cargo() {
        let local = pkg("app", "0.1.0", None);
        assert_eq!(
            package_id(&local, Some(Path::new("/w/app"))),
            "path+file:///w/app#0.1.0"
        );
        assert_eq!(
            package_id(&local, Some(Path::new("/w/other"))),
            "path+file:///w/other#app@0.1.0"
        );
        // Windows drive paths, plain and as `canonicalize` returns them.
        assert_eq!(
            path_source_url(Path::new(r"C:\w\app")),
            "path+file:///C:/w/app"
        );
        assert_eq!(
            path_source_url(Path::new(r"\\?\C:\w\app")),
            "path+file:///C:/w/app"
        );
        let reg = pkg(
            "serde",
            "1.0.200",
            Some("registry+https://github.com/rust-lang/crates.io-index"),
        );
        assert_eq!(
            package_id(&reg, None),
            "registry+https://github.com/rust-lang/crates.io-index#serde@1.0.200"
        );
        let git = pkg(
            "x",
            "0.2.0",
            Some("git+https://example.com/x?branch=main#abc123"),
        );
        assert_eq!(
            package_id(&git, None),
            "git+https://example.com/x?branch=main#x@0.2.0"
        );
    }

    #[test]
    fn specs_take_partial_versions() {
        assert_eq!(parse_spec("serde@1.0"), ("serde", Some("1.0")));
        assert_eq!(parse_spec("serde:1"), ("serde", Some("1")));
        assert!(version_matches("1.0.200", "1"));
        assert!(version_matches("1.0.200", "1.0.200"));
        assert!(!version_matches("1.0.200", "1.1"));
    }
}
