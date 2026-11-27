/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! `mitts generate-lockfile` and `mitts update`: Cargo.lock written by Mitts' own resolver,
//! with Cargo's selection rules (`-p`, `--precise`, `--recursive`, `--workspace`,
//! `--dry-run`) and its report of what moved.

use crate::adapter::rust::lockfile::{CargoLock, CargoLockPackage};
use crate::adapter::rust::resolver::NativeResolver;
use anyhow::{bail, Context, Result};
use clap::Args;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Args, Debug, Clone, Default)]
pub struct GenerateLockfileArgs {
    /// Path to Cargo.toml
    #[arg(long)]
    pub manifest_path: Option<PathBuf>,

    /// Pick versions whatever their `rust-version` (resolver.incompatible-rust-versions = allow)
    #[arg(long)]
    pub ignore_rust_version: bool,

    /// Directory for the shared artifact and action cache
    #[arg(long)]
    pub cache_dir: Option<PathBuf>,

    /// Run without accessing the network
    #[arg(long)]
    pub offline: bool,

    /// Require Cargo.lock is up to date (an error for this command)
    #[arg(long)]
    pub locked: bool,

    /// Equivalent to specifying both --locked and --offline
    #[arg(long)]
    pub frozen: bool,

    /// Do not print Mitts log messages
    #[arg(short, long)]
    pub quiet: bool,

    /// Use verbose output
    #[arg(short, long, action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// Latest publish time allowed for registry packages (unstable in Cargo)
    #[arg(long, value_name = "TIME")]
    pub publish_time: Option<String>,
}

#[derive(Args, Debug, Clone, Default)]
pub struct UpdateArgs {
    /// Packages to update
    #[arg(value_name = "SPEC")]
    pub specs: Vec<String>,

    /// Package to update
    #[arg(short = 'p', long = "package", value_name = "SPEC", action = clap::ArgAction::Append)]
    pub packages: Vec<String>,

    /// Update a single dependency to exactly PRECISE when used with a single package
    #[arg(long, value_name = "PRECISE")]
    pub precise: Option<String>,

    /// Force updating all dependencies of the selected packages as well
    #[arg(long, visible_alias = "aggressive")]
    pub recursive: bool,

    /// Only update the workspace packages
    #[arg(short = 'w', long)]
    pub workspace: bool,

    /// Don't actually write the lockfile
    #[arg(long)]
    pub dry_run: bool,

    #[command(flatten)]
    pub common: GenerateLockfileArgs,
}

pub fn status(word: &str, message: &str) {
    eprintln!("{word:>12} {message}");
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

fn refuse_locked(common: &GenerateLockfileArgs, root: &Path) -> Result<()> {
    if common.locked || common.frozen {
        let flag = if common.frozen {
            "--frozen"
        } else {
            "--locked"
        };
        bail!(
            "the lock file {} needs to be updated but {flag} was passed to prevent this",
            root.with_file_name("Cargo.lock").display()
        );
    }
    Ok(())
}

pub fn handle_generate_lockfile(args: &GenerateLockfileArgs) -> i32 {
    report((|| {
        if args.publish_time.is_some() {
            bail!("`--publish-time` is unstable in Cargo and not supported by Mitts");
        }
        let root = workspace_root_manifest(&args.manifest_path)?;
        refuse_locked(args, &root)?;
        let previous = CargoLock::from_file(&root.with_file_name("Cargo.lock")).ok();
        let mut lock = resolve_precise(&root, None, None, &[], args)?;
        if let Some(prev) = &previous {
            lock.version = prev.version;
        }
        let changes = diff(previous.as_ref(), &lock);
        if !args.quiet {
            print_locking(&lock, previous.as_ref());
            print_changes(&changes, previous.is_some());
        }
        lock.write_to_file(&root.with_file_name("Cargo.lock"))
    })())
}

pub fn handle_update(args: &UpdateArgs) -> i32 {
    report(run_update(args))
}

fn run_update(args: &UpdateArgs) -> Result<()> {
    let common = &args.common;
    let root = workspace_root_manifest(&common.manifest_path)?;
    let lock_path = root.with_file_name("Cargo.lock");
    let previous = CargoLock::from_file(&lock_path).ok();
    let mut specs: Vec<String> = args.specs.clone();
    specs.extend(args.packages.iter().cloned());
    if args.precise.is_some() && specs.len() != 1 {
        bail!("--precise requires exactly one package");
    }
    // What stays pinned: everything (workspace-only), everything but the selected
    // packages, or nothing (a plain update re-resolves the whole graph).
    // With --precise the package keeps its lock entry: its source tells the resolver
    // where the exact version comes from.
    let keep = match (
        &previous,
        args.workspace || args.precise.is_some(),
        specs.is_empty(),
    ) {
        (Some(prev), true, _) => Some(prev.clone()),
        (Some(prev), false, false) => Some(without(prev, &specs, args.recursive)?),
        _ => None,
    };
    let mut lock = resolve_precise(
        &root,
        keep.as_ref(),
        args.precise.as_deref(),
        &specs,
        common,
    )?;
    if let Some(prev) = &previous {
        lock.version = prev.version;
    }
    let changes = diff(previous.as_ref(), &lock);
    // The whole lock decides, not only names and versions: a source, checksum or
    // dependency list that moved is a change too.
    let unchanged = previous
        .as_ref()
        .is_some_and(|prev| prev.to_string_formatted() == lock.to_string_formatted());
    if !unchanged {
        refuse_locked(common, &root)?;
    }
    if !common.quiet {
        print_locking(&lock, previous.as_ref());
        print_changes(&changes, true);
    }
    if args.dry_run {
        eprintln!("warning: aborting update due to dry run");
        return Ok(());
    }
    if unchanged {
        return Ok(());
    }
    lock.write_to_file(&lock_path)
}

/// The workspace root's manifest for `--manifest-path` or the current directory.
pub fn workspace_root_manifest(explicit: &Option<PathBuf>) -> Result<PathBuf> {
    let start = super::project_info::starting_manifest(explicit)?;
    super::project_info::workspace_root(&start)
}

/// The lock with the packages `specs` name (and, `recursive`, everything below them)
/// taken out, so the resolver picks them afresh while the rest stays where it is.
fn without(lock: &CargoLock, specs: &[String], recursive: bool) -> Result<CargoLock> {
    let mut drop: HashSet<(String, String)> = HashSet::new();
    for spec in specs {
        let (name, version) = super::project_info::parse_spec(spec);
        let found: Vec<&CargoLockPackage> = lock
            .packages
            .iter()
            .filter(|p| p.name == name && version.is_none_or(|v| p.version.starts_with(v)))
            .collect();
        if found.is_empty() {
            bail!("package ID specification `{spec}` did not match any packages");
        }
        for p in found {
            drop.insert((p.name.clone(), p.version.clone()));
        }
    }
    if recursive {
        let mut queue: Vec<(String, String)> = drop.iter().cloned().collect();
        while let Some((name, version)) = queue.pop() {
            let Some(pkg) = lock
                .packages
                .iter()
                .find(|p| p.name == name && p.version == version)
            else {
                continue;
            };
            for dep in &pkg.dependencies {
                let mut words = dep.split(' ');
                let dep_name = words.next().unwrap_or_default();
                let dep_version = words.next();
                for p in lock
                    .packages
                    .iter()
                    .filter(|p| p.name == dep_name && dep_version.is_none_or(|v| p.version == v))
                {
                    if drop.insert((p.name.clone(), p.version.clone())) {
                        queue.push((p.name.clone(), p.version.clone()));
                    }
                }
            }
        }
    }
    let mut kept = lock.clone();
    kept.packages
        .retain(|p| !drop.contains(&(p.name.clone(), p.version.clone())));
    Ok(kept)
}

fn resolve_precise(
    root: &Path,
    keep: Option<&CargoLock>,
    precise: Option<&str>,
    specs: &[String],
    args: &GenerateLockfileArgs,
) -> Result<CargoLock> {
    let offline = args.offline || args.frozen;
    if !offline && !args.quiet {
        status("Updating", "crates.io index");
    }
    let mut resolver = NativeResolver::for_cache_dir(args.cache_dir.as_deref())?;
    if let Some(keep) = keep {
        resolver = resolver.prefer_locked(keep);
    }
    let precise_target = match (precise, specs.first()) {
        (Some(version), Some(spec)) => {
            let (name, _) = super::project_info::parse_spec(spec);
            Some((name.to_string(), version.to_string()))
        }
        _ => None,
    };
    if let Some((name, version)) = &precise_target {
        resolver = resolver.prefer_version(name, version);
    }
    let lock = resolver
        .resolve_manifest(root, offline)
        .context("failed to resolve dependencies")?;
    if let Some((name, version)) = &precise_target {
        if !lock
            .packages
            .iter()
            .any(|p| p.name == *name && p.version == *version)
        {
            bail!(
                "failed to select a version for the requirement `{name} = \"={version}\"`: \
                 {name} {version} does not satisfy the requirements of the dependency graph \
                 or is not in the registry"
            );
        }
    }
    Ok(lock)
}

#[derive(Debug, PartialEq, Eq)]
pub enum Change {
    Updating(String, String, String),
    Adding(String, String),
    Removing(String, String),
}

/// What moved between two locks, by package name, in Cargo's terms: one version replaced
/// by another is an update, everything else an addition or a removal.
pub fn diff(old: Option<&CargoLock>, new: &CargoLock) -> Vec<Change> {
    let versions = |lock: &CargoLock| {
        let mut map: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for p in lock.packages.iter().filter(|p| p.source.is_some()) {
            map.entry(p.name.clone())
                .or_default()
                .push(p.version.clone());
        }
        map
    };
    let before = old.map(versions).unwrap_or_default();
    let after = versions(new);
    let mut names: Vec<&String> = before.keys().chain(after.keys()).collect();
    names.sort();
    names.dedup();
    let mut out = Vec::new();
    for name in names {
        let empty = Vec::new();
        let b = before.get(name).unwrap_or(&empty);
        let a = after.get(name).unwrap_or(&empty);
        let gone: Vec<&String> = b.iter().filter(|v| !a.contains(v)).collect();
        let came: Vec<&String> = a.iter().filter(|v| !b.contains(v)).collect();
        if gone.len() == 1 && came.len() == 1 {
            out.push(Change::Updating(
                name.clone(),
                gone[0].clone(),
                came[0].clone(),
            ));
            continue;
        }
        out.extend(
            gone.into_iter()
                .map(|v| Change::Removing(name.clone(), v.clone())),
        );
        out.extend(
            came.into_iter()
                .map(|v| Change::Adding(name.clone(), v.clone())),
        );
    }
    out
}

fn print_locking(lock: &CargoLock, previous: Option<&CargoLock>) {
    let changed = diff(previous, lock)
        .iter()
        .filter(|c| !matches!(c, Change::Removing(..)))
        .count();
    let noun = if changed == 1 { "package" } else { "packages" };
    status(
        "Locking",
        &format!("{changed} {noun} to latest compatible versions"),
    );
}

fn print_changes(changes: &[Change], show_adding: bool) {
    for change in changes {
        match change {
            Change::Updating(name, from, to) => {
                status("Updating", &format!("{name} v{from} -> v{to}"))
            }
            Change::Adding(name, version) if show_adding => {
                status("Adding", &format!("{name} v{version}"))
            }
            Change::Adding(..) => {}
            Change::Removing(name, version) => status("Removing", &format!("{name} v{version}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lock(pkgs: &[(&str, &str)]) -> CargoLock {
        CargoLock {
            version: 4,
            packages: pkgs
                .iter()
                .map(|(n, v)| CargoLockPackage {
                    name: n.to_string(),
                    version: v.to_string(),
                    source: Some("registry+https://github.com/rust-lang/crates.io-index".into()),
                    checksum: None,
                    dependencies: Vec::new(),
                })
                .collect(),
        }
    }

    #[test]
    fn changes_are_named_like_cargo() {
        let old = lock(&[("a", "1.0.0"), ("b", "1.0.0"), ("c", "1.0.0")]);
        let new = lock(&[("a", "1.0.1"), ("c", "1.0.0"), ("d", "2.0.0")]);
        assert_eq!(
            diff(Some(&old), &new),
            [
                Change::Updating("a".into(), "1.0.0".into(), "1.0.1".into()),
                Change::Removing("b".into(), "1.0.0".into()),
                Change::Adding("d".into(), "2.0.0".into()),
            ]
        );
    }

    #[test]
    fn a_selected_package_and_with_recursive_its_dependencies_are_freed() {
        let mut old = lock(&[("a", "1.0.0"), ("b", "1.0.0"), ("c", "1.0.0")]);
        old.packages[0].dependencies = vec!["b".into()];
        let kept = without(&old, &["a".into()], false).unwrap();
        assert_eq!(kept.packages.len(), 2);
        let kept = without(&old, &["a".into()], true).unwrap();
        assert_eq!(kept.packages.len(), 1);
        assert!(without(&old, &["zzz".into()], false).is_err());
    }
}
