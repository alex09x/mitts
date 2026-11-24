/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! The checkout of a registry whose index is a git repository (`registry+<url>` other than
//! crates.io). It is cloned shallow with the system git (credentials as git has them, as
//! Cargo's `net.git-fetch-with-cli`), refreshed when older than the index TTL, and read in
//! place: the files have the sparse index layout. One process updates it at a time.

use crate::core::lock::FileLock;
use anyhow::{bail, Context, Result};
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

/// The marker whose modification time says when the checkout was last brought up to date.
const FETCHED: &str = ".mitts-fetched";

fn git(dir: Option<&Path>, args: &[&str]) -> Result<()> {
    let bin = std::env::var("MITTS_GIT").unwrap_or_else(|_| "git".to_string());
    let mut cmd = Command::new(bin);
    if let Some(d) = dir {
        cmd.current_dir(d);
    }
    // Index files are read as committed: no line-ending conversion on Windows.
    let out = cmd
        .args(["-c", "core.autocrlf=false"])
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .with_context(|| format!("running git {}", args.join(" ")))?;
    if !out.status.success() {
        bail!(
            "git {} failed ({}): {}",
            args.join(" "),
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

fn fresh(checkout: &Path, ttl: Duration) -> bool {
    checkout
        .join(FETCHED)
        .metadata()
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age < ttl)
}

/// Makes `checkout` a current shallow clone of the index at `url`. Offline, an existing
/// checkout is used as it is; without one the build stops, as Cargo's does.
pub fn ensure_checkout(url: &str, checkout: &Path, offline: bool, ttl: Duration) -> Result<()> {
    let exists = checkout.join(".git").is_dir();
    if exists && (offline || fresh(checkout, ttl)) {
        return Ok(());
    }
    if offline {
        bail!("the index of registry {url} is not in the cache and --offline was given");
    }
    let parent = checkout.parent().context("index checkout has no parent")?;
    fs::create_dir_all(parent)?;
    let _lock = FileLock::acquire_exclusive(&checkout.with_extension("lock"))?;
    // Another process may have done it while this one waited.
    if checkout.join(".git").is_dir() && fresh(checkout, ttl) {
        return Ok(());
    }
    if checkout.join(".git").is_dir() {
        let updated =
            git(Some(checkout), &["fetch", "--depth", "1", "origin", "HEAD"]).and_then(|()| {
                git(
                    Some(checkout),
                    &["reset", "--hard", "--quiet", "FETCH_HEAD"],
                )
            });
        match updated {
            Ok(()) => {}
            // A stale index still resolves what the lockfile names; say what failed.
            Err(e) => {
                eprintln!("[mitts] warning: could not update the index of {url}: {e:#}");
                return Ok(());
            }
        }
    } else {
        let staging = checkout.with_extension(format!("clone-{}", std::process::id()));
        let _ = fs::remove_dir_all(&staging);
        let staging_str = staging.to_string_lossy().into_owned();
        git(
            None,
            &["clone", "--quiet", "--depth", "1", url, &staging_str],
        )
        .with_context(|| format!("cloning the index of registry {url}"))?;
        if checkout.exists() {
            fs::remove_dir_all(checkout)?;
        }
        fs::rename(&staging, checkout)
            .with_context(|| format!("moving the index checkout into {}", checkout.display()))?;
    }
    fs::write(checkout.join(FETCHED), "")?;
    Ok(())
}
