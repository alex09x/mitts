/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! `mitts vendor`: every registry and git dependency copied into a directory with Cargo's
//! `.cargo-checksum.json`, and the `[source]` configuration that uses it printed on
//! stdout, as `cargo vendor` does.

use crate::adapter::rust::fetcher::FetchOptions;
use crate::adapter::rust::lockfile::{CargoLock, CargoLockPackage};
use crate::adapter::rust::source_lookup::find_crate_source_dir_with_store;
use crate::adapter::RustAdapter;
use crate::core::source_store::SourceStore;
use anyhow::{bail, Context, Result};
use clap::Args;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

#[derive(Args, Debug, Clone, Default)]
pub struct VendorArgs {
    /// Where to vendor crates (`vendor` by default)
    #[arg(value_name = "PATH")]
    pub path: Option<PathBuf>,

    /// Additional `Cargo.toml` to sync and vendor
    #[arg(short = 's', long = "sync", value_name = "TOML", action = clap::ArgAction::Append)]
    pub sync: Vec<PathBuf>,

    /// Don't delete older crates in the vendor directory
    #[arg(long)]
    pub no_delete: bool,

    /// Respect `[source]` config in `.cargo/config`
    #[arg(long)]
    pub respect_source_config: bool,

    /// Always include version in subdir name
    #[arg(long)]
    pub versioned_dirs: bool,

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

pub fn handle_vendor(args: &VendorArgs) -> i32 {
    match run(args) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("error: {error:#}");
            101
        }
    }
}

fn status(word: &str, message: &str) {
    eprintln!("{word:>12} {message}");
}

/// Files of an unpacked package that are not part of it.
fn skipped(rel: &Path, git: bool) -> bool {
    let first = rel
        .components()
        .next()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .unwrap_or_default();
    matches!(
        first.as_str(),
        ".mitts-ok" | ".cargo-ok" | ".cargo-checksum.json" | ".git"
    ) || (git && first == "target")
}

fn run(args: &VendorArgs) -> Result<()> {
    let root = super::lock_cmds::workspace_root_manifest(&args.manifest_path)?;
    let ws_dir = root.parent().unwrap_or(Path::new(".")).to_path_buf();
    let dest_rel = args.path.clone().unwrap_or_else(|| PathBuf::from("vendor"));
    let cwd = std::env::current_dir()?;
    let dest = cwd.join(&dest_rel);
    let store = match &args.cache_dir {
        Some(dir) => SourceStore::new(dir.join("sources"))?,
        None => SourceStore::default_store()?,
    };
    let fetch_opt = FetchOptions {
        offline: args.offline,
        locked: args.locked,
        frozen: args.frozen,
    };
    let mut manifests = vec![root.clone()];
    manifests.extend(args.sync.iter().map(|m| cwd.join(m)));
    // (name, version) -> (package, source dir)
    let mut packages: BTreeMap<(String, String), (CargoLockPackage, PathBuf)> = BTreeMap::new();
    for manifest in &manifests {
        RustAdapter::fetch(manifest, &store, &fetch_opt, args.cache_dir.as_deref())
            .with_context(|| format!("failed to fetch the dependencies of {}", manifest.display()))?;
        let lock = CargoLock::from_file(&manifest.with_file_name("Cargo.lock"))?;
        for pkg in lock.packages.into_iter().filter(|p| p.source.is_some()) {
            let dir = find_crate_source_dir_with_store(
                &pkg.name,
                &pkg.version,
                pkg.source.as_deref(),
                Some(&store),
            )
            .with_context(|| format!("the source of {} v{} is not available", pkg.name, pkg.version))?;
            packages.insert((pkg.name.clone(), pkg.version.clone()), (pkg, dir));
        }
    }
    let mut counts: HashMap<String, usize> = HashMap::new();
    for (name, _) in packages.keys() {
        *counts.entry(name.clone()).or_default() += 1;
    }

    std::fs::create_dir_all(&dest)?;
    let mut written: BTreeSet<String> = BTreeSet::new();
    let mut git_sources: BTreeSet<String> = BTreeSet::new();
    for ((name, version), (pkg, src)) in &packages {
        let dir_name = if args.versioned_dirs || counts[name] > 1 {
            format!("{name}-{version}")
        } else {
            name.clone()
        };
        let source = pkg.source.as_deref().unwrap_or_default();
        let git = source.starts_with("git+");
        if git {
            git_sources.insert(source.split('#').next().unwrap_or(source).to_string());
        }
        let target = dest.join(&dir_name);
        if !args.quiet {
            status(
                "Vendoring",
                &format!("{name} v{version} ({}) to {}", src.display(), dest_rel.join(&dir_name).display()),
            );
        }
        if target.exists() {
            std::fs::remove_dir_all(&target)?;
        }
        let mut files: BTreeMap<String, String> = BTreeMap::new();
        copy_tree(src, src, &target, git, &mut files)?;
        let checksum = serde_json::json!({
            "files": files,
            "package": if git { serde_json::Value::Null } else { pkg.checksum.clone().map(serde_json::Value::String).unwrap_or(serde_json::Value::Null) },
        });
        std::fs::write(target.join(".cargo-checksum.json"), checksum.to_string())?;
        written.insert(dir_name);
    }
    if !args.no_delete {
        for entry in std::fs::read_dir(&dest)?.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if entry.path().is_dir()
                && !written.contains(&name)
                && entry.path().join(".cargo-checksum.json").is_file()
            {
                std::fs::remove_dir_all(entry.path())?;
            }
        }
    }

    let shown = dest
        .strip_prefix(&ws_dir)
        .map(Path::to_path_buf)
        .unwrap_or_else(|_| dest_rel.clone());
    let mut config = String::from(
        "[source.crates-io]\nreplace-with = \"vendored-sources\"\n\n",
    );
    for url in &git_sources {
        let (base, query) = url.trim_start_matches("git+").split_once('?').unwrap_or((url.trim_start_matches("git+"), ""));
        config.push_str(&format!("[source.\"{url}\"]\ngit = \"{base}\"\n"));
        for pair in query.split('&').filter(|p| !p.is_empty()) {
            if let Some((k, v)) = pair.split_once('=') {
                config.push_str(&format!("{k} = \"{v}\"\n"));
            }
        }
        config.push_str("replace-with = \"vendored-sources\"\n\n");
    }
    config.push_str(&format!(
        "[source.vendored-sources]\ndirectory = \"{}\"\n",
        shown.display().to_string().replace('\\', "/")
    ));
    if !args.quiet {
        eprintln!("To use vendored sources, add this to your .cargo/config.toml for this project:\n");
    }
    print!("{config}");
    Ok(())
}

fn copy_tree(
    base: &Path,
    dir: &Path,
    dest: &Path,
    git: bool,
    files: &mut BTreeMap<String, String>,
) -> Result<()> {
    std::fs::create_dir_all(dest)?;
    let mut entries: Vec<_> = std::fs::read_dir(dir)?.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        let rel = path.strip_prefix(base).unwrap_or(&path).to_path_buf();
        if skipped(&rel, git) {
            continue;
        }
        let kind = entry.file_type()?;
        let out = dest.join(entry.file_name());
        if kind.is_dir() {
            copy_tree(base, &path, &out, git, files)?;
        } else if kind.is_file() || kind.is_symlink() {
            let data = std::fs::read(&path).with_context(|| format!("read {}", path.display()))?;
            std::fs::write(&out, &data)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(meta) = std::fs::metadata(&path) {
                    let mode = meta.permissions().mode() | 0o200;
                    let _ = std::fs::set_permissions(&out, std::fs::Permissions::from_mode(mode));
                }
            }
            files.insert(
                rel.to_string_lossy().replace('\\', "/"),
                crate::core::hash::Sha256::digest(&data),
            );
        }
    }
    if files.is_empty() && dir == base {
        bail!("{} has no files", base.display());
    }
    Ok(())
}
