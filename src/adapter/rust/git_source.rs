/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Git dependencies. Mitts clones them itself with the system `git`; Cargo is never run.
//!
//! Layout under `<cache root>/git/`:
//!
//! ```text
//! db/<repo>-<url digest>/                 bare mirror of the remote, fetched on demand
//! checkouts/<repo>-<url digest>/<rev7>/   working tree at the locked commit; `.mitts-ok` once complete
//! ```
//!
//! Authentication is whatever the system git has: SSH keys and agents, credential helpers,
//! `GIT_SSH_COMMAND`, `~/.gitconfig`. Prompts are disabled so a missing credential fails
//! instead of hanging a build. `MITTS_GIT` names an alternative git binary.

use anyhow::{anyhow, bail, Context, Result};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use crate::core::cache_root::{self, GIT_DIR};
use crate::core::lock::FileLock;
use crate::core::source_store::MITTS_OK_MARKER;

/// What the manifest asked for; the lockfile pins the commit regardless.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitReference {
    Tag(String),
    Branch(String),
    Rev(String),
    Default,
}

/// A `Cargo.lock` source of the form `git+<url>?tag=<t>#<commit>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitSource {
    pub url: String,
    pub reference: GitReference,
    /// The locked commit, as written by the resolver (a full SHA).
    pub precise: String,
}

impl GitSource {
    pub fn parse(source: &str) -> Option<Self> {
        let rest = source.strip_prefix("git+")?;
        let (spec, precise) = rest.split_once('#')?;
        if precise.is_empty() {
            return None;
        }
        let (url, query) = match spec.split_once('?') {
            Some((u, q)) => (u, Some(q)),
            None => (spec, None),
        };
        let mut reference = GitReference::Default;
        for pair in query.unwrap_or("").split('&') {
            let Some((k, v)) = pair.split_once('=') else {
                continue;
            };
            reference = match k {
                "tag" => GitReference::Tag(v.to_string()),
                "branch" => GitReference::Branch(v.to_string()),
                "rev" => GitReference::Rev(v.to_string()),
                _ => continue,
            };
        }
        Some(Self {
            url: url.to_string(),
            reference,
            precise: precise.to_string(),
        })
    }

    fn canonical_url(&self) -> String {
        let trimmed = self.url.trim_end_matches('/');
        trimmed.strip_suffix(".git").unwrap_or(trimmed).to_string()
    }

    /// Last path segment of the URL without `.git`: `order-book-rs` for
    /// `ssh://git@github.com/org/order-book-rs.git`.
    pub fn repo_name(&self) -> String {
        let canonical = self.canonical_url();
        let name = canonical
            .rsplit(['/', ':'])
            .find(|s| !s.is_empty())
            .unwrap_or("repo");
        let safe: String = name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        if safe.is_empty() {
            "repo".to_string()
        } else {
            safe
        }
    }

    /// Directory name shared by the mirror and its checkouts: `<repo>-<16 hex of sha256(url)>`.
    pub fn repo_dir_name(&self) -> String {
        let digest = Sha256::digest(self.canonical_url().as_bytes());
        let short: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
        format!("{}-{}", self.repo_name(), short)
    }

    /// Cargo's seven-character checkout directory name.
    pub fn short_rev(&self) -> &str {
        &self.precise[..self.precise.len().min(7)]
    }

    /// The `source` string Cargo writes for this dependency in `Cargo.lock`.
    pub fn lock_source(&self) -> String {
        let query = match &self.reference {
            GitReference::Tag(t) => format!("?tag={t}"),
            GitReference::Branch(b) => format!("?branch={b}"),
            GitReference::Rev(r) => format!("?rev={r}"),
            GitReference::Default => String::new(),
        };
        format!("git+{}{}#{}", self.url, query, self.precise)
    }
}

/// `git ls-remote <url> <patterns…>`: `(commit, ref name)` pairs as the server advertises
/// them. An annotated tag's peeled entry (`refs/tags/x^{}`) is listed only when asked for.
pub fn ls_remote(url: &str, patterns: &[&str]) -> Result<Vec<(String, String)>> {
    let mut args = vec!["ls-remote", url];
    args.extend_from_slice(patterns);
    let out = run_git(None, &args)?;
    Ok(out
        .lines()
        .filter_map(|l| {
            let (sha, name) = l.split_once('\t')?;
            Some((sha.trim().to_string(), name.trim().to_string()))
        })
        .collect())
}

/// Turns a manifest's tag, branch or revision into the commit the lockfile will pin.
/// Annotated tags resolve to the tagged commit (the `^{}` peeled entry), as Cargo does.
pub fn resolve_reference(
    store: &GitSourceStore,
    url: &str,
    reference: &GitReference,
    offline: bool,
) -> Result<String> {
    let is_full_sha = |s: &str| s.len() == 40 && s.chars().all(|c| c.is_ascii_hexdigit());
    if let GitReference::Rev(r) = reference {
        if is_full_sha(r) {
            return Ok(r.to_lowercase());
        }
    }
    if offline {
        bail!(
            "git dependency {url}: resolving {reference:?} to a commit needs the network and offline mode was requested"
        );
    }
    let pattern = match reference {
        GitReference::Tag(t) => format!("refs/tags/{t}"),
        GitReference::Branch(b) => format!("refs/heads/{b}"),
        GitReference::Default => "HEAD".to_string(),
        GitReference::Rev(r) => return store.resolve_rev(url, r),
    };
    let peeled = format!("{pattern}^{{}}");
    let found = ls_remote(url, &[&pattern, &peeled])
        .with_context(|| format!("git dependency {url}: listing {pattern}"))?;
    let pick = found
        .iter()
        .find(|(_, name)| *name == peeled)
        .or_else(|| found.iter().find(|(_, name)| *name == pattern))
        .or_else(|| found.first())
        .map(|(sha, _)| sha.clone());
    pick.ok_or_else(|| anyhow!("git dependency {url}: {pattern} not found on the remote"))
}

/// The git half of a cache root: mirrors and checkouts under `<cache root>/git/`.
#[derive(Debug, Clone)]
pub struct GitSourceStore {
    root: PathBuf,
}

impl GitSourceStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// `<default cache root>/git` (see [`crate::core::cache_root`] for the precedence).
    pub fn default_root() -> PathBuf {
        cache_root::default_root().join(GIT_DIR)
    }

    /// The git store of the cache root a source store belongs to: `<root>/sources` → `<root>/git`.
    pub fn beside_sources(sources_root: &Path) -> Self {
        let root = sources_root
            .parent()
            .map(|p| p.join(GIT_DIR))
            .unwrap_or_else(Self::default_root);
        Self { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn checkouts_dir(&self) -> PathBuf {
        self.root.join("checkouts")
    }

    fn db_dir(&self, src: &GitSource) -> PathBuf {
        self.root.join("db").join(src.repo_dir_name())
    }

    /// `checkouts/<repo>-<digest>/<rev7>`: the directory a complete checkout lives in.
    pub fn checkout_dir(&self, src: &GitSource) -> PathBuf {
        self.checkouts_dir()
            .join(src.repo_dir_name())
            .join(src.short_rev())
    }

    pub fn has_checkout(&self, src: &GitSource) -> bool {
        self.checkout_dir(src).join(MITTS_OK_MARKER).is_file()
    }

    /// Resolves an abbreviated or symbolic `rev` by fetching every ref into the mirror and
    /// asking git; a remote cannot answer `ls-remote` for a partial SHA.
    pub fn resolve_rev(&self, url: &str, rev: &str) -> Result<String> {
        let probe = GitSource {
            url: url.to_string(),
            reference: GitReference::Rev(rev.to_string()),
            precise: rev.to_string(),
        };
        let db = self.db_dir(&probe);
        let locks = self.root.join(".locks");
        fs::create_dir_all(&locks)?;
        let _lock = FileLock::acquire_exclusive_timeout(
            &locks.join(format!("{}.lock", probe.repo_dir_name())),
            Duration::from_secs(1800),
        )?;
        if !db.join("HEAD").is_file() {
            fs::create_dir_all(&db)?;
            run_git(None, &["init", "--quiet", "--bare", &db.to_string_lossy()])?;
        }
        run_git(
            Some(&db),
            &[
                "fetch",
                "--quiet",
                "--tags",
                url,
                "+refs/heads/*:refs/remotes/origin/*",
            ],
        )
        .with_context(|| format!("git dependency {url}: fetching to resolve rev {rev}"))?;
        let sha = run_git(
            Some(&db),
            &["rev-parse", "--verify", &format!("{rev}^{{commit}}")],
        )
        .with_context(|| format!("git dependency {url}: rev {rev} not found"))?;
        Ok(sha.trim().to_string())
    }

    /// Returns the checkout of the locked commit, cloning or fetching it first unless `offline`.
    pub fn ensure_checkout(&self, src: &GitSource, offline: bool) -> Result<PathBuf> {
        let dest = self.checkout_dir(src);
        if dest.join(MITTS_OK_MARKER).is_file() {
            return Ok(dest);
        }
        let locks = self.root.join(".locks");
        fs::create_dir_all(&locks)?;
        let lock_path = locks.join(format!("{}.lock", src.repo_dir_name()));
        let _lock = FileLock::acquire_exclusive_timeout(&lock_path, Duration::from_secs(1800))
            .with_context(|| format!("waiting for another Mitts fetching {}", src.url))?;
        if dest.join(MITTS_OK_MARKER).is_file() {
            return Ok(dest);
        }

        let db = self.db_dir(src);
        if !db_has_commit(&db, &src.precise) {
            if offline {
                bail!(
                    "git dependency {} at commit {} is not available locally and offline mode was requested",
                    src.url,
                    src.precise
                );
            }
            fetch_into_db(src, &db)?;
        }

        let parent = dest.parent().expect("checkout dir has a parent");
        fs::create_dir_all(parent)?;
        let staging = parent.join(format!(
            ".staging-{}-{}",
            src.short_rev(),
            std::process::id()
        ));
        if staging.exists() {
            fs::remove_dir_all(&staging)?;
        }
        run_git(
            None,
            &[
                "clone",
                "--quiet",
                "--no-checkout",
                &db.to_string_lossy(),
                &staging.to_string_lossy(),
            ],
        )?;
        run_git(
            Some(&staging),
            &["checkout", "--quiet", "--detach", &src.precise],
        )?;
        if staging.join(".gitmodules").is_file() {
            run_git(
                Some(&staging),
                &["submodule", "update", "--init", "--recursive", "--quiet"],
            )
            .with_context(|| format!("submodules of {}", src.url))?;
        }
        fs::write(staging.join(MITTS_OK_MARKER), "")?;
        match fs::rename(&staging, &dest) {
            Ok(()) => {}
            Err(_) if dest.join(MITTS_OK_MARKER).is_file() => {
                let _ = fs::remove_dir_all(&staging);
            }
            Err(e) => {
                let _ = fs::remove_dir_all(&staging);
                return Err(anyhow!(e)).with_context(|| format!("publishing checkout {:?}", dest));
            }
        }
        Ok(dest)
    }
}

fn db_has_commit(db: &Path, rev: &str) -> bool {
    db.join("HEAD").is_file()
        && git_command(Some(db))
            .args(["cat-file", "-e", &format!("{rev}^{{commit}}")])
            .output()
            .is_ok_and(|o| o.status.success())
}

/// Brings the locked commit into the bare mirror: the exact commit when the server allows it
/// (GitHub, GitLab, modern git), else the named tag or branch, else everything.
fn fetch_into_db(src: &GitSource, db: &Path) -> Result<()> {
    if !db.join("HEAD").is_file() {
        fs::create_dir_all(db)?;
        run_git(None, &["init", "--quiet", "--bare", &db.to_string_lossy()])?;
    }
    let mut last_err = None;
    let mut attempt = |refspec: &str| -> bool {
        match run_git(Some(db), &["fetch", "--quiet", &src.url, refspec]) {
            Ok(_) => {}
            Err(e) => last_err = Some(e),
        }
        db_has_commit(db, &src.precise)
    };
    if attempt(&src.precise) {
        return Ok(());
    }
    let named = match &src.reference {
        GitReference::Tag(t) => Some(format!("+refs/tags/{t}:refs/tags/{t}")),
        GitReference::Branch(b) => Some(format!("+refs/heads/{b}:refs/remotes/origin/{b}")),
        GitReference::Rev(_) | GitReference::Default => None,
    };
    if let Some(spec) = named {
        if attempt(&spec) {
            return Ok(());
        }
    }
    let everything = run_git(
        Some(db),
        &[
            "fetch",
            "--quiet",
            "--tags",
            &src.url,
            "+refs/heads/*:refs/remotes/origin/*",
        ],
    );
    if db_has_commit(db, &src.precise) {
        return Ok(());
    }
    let detail = everything
        .err()
        .or(last_err)
        .map(|e| format!(": {e:#}"))
        .unwrap_or_default();
    bail!(
        "git dependency {}: commit {} could not be fetched{}",
        src.url,
        src.precise,
        detail
    )
}

fn git_command(cwd: Option<&Path>) -> Command {
    let bin = std::env::var("MITTS_GIT").unwrap_or_else(|_| "git".to_string());
    let mut cmd = Command::new(bin);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    cmd.env("GIT_TERMINAL_PROMPT", "0")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE");
    cmd
}

fn run_git(cwd: Option<&Path>, args: &[&str]) -> Result<String> {
    let out = git_command(cwd)
        .args(args)
        .output()
        .with_context(|| format!("running git {}", args.join(" ")))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        bail!(
            "git {} failed ({}): {}",
            args.join(" "),
            out.status,
            stderr.trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_lockfile_sources() {
        let s = GitSource::parse(
            "git+ssh://git@github.com/org/order-book-rs.git?tag=1.3.0#b4efb0f7021f828548ee5a9aff655c5ba3068633",
        )
        .unwrap();
        assert_eq!(s.url, "ssh://git@github.com/org/order-book-rs.git");
        assert_eq!(s.reference, GitReference::Tag("1.3.0".into()));
        assert_eq!(s.short_rev(), "b4efb0f");
        assert_eq!(s.repo_name(), "order-book-rs");
        let plain = GitSource::parse("git+https://github.com/a/b#0123456789abcdef").unwrap();
        assert_eq!(plain.reference, GitReference::Default);
        assert!(GitSource::parse("git+https://github.com/a/b").is_none());
        assert!(GitSource::parse("registry+https://x#abc").is_none());
    }

    #[test]
    fn repo_dir_name_ignores_dot_git_and_trailing_slash() {
        let a = GitSource::parse("git+https://github.com/a/b.git#1").unwrap();
        let b = GitSource::parse("git+https://github.com/a/b/#1").unwrap();
        assert_eq!(a.repo_dir_name(), b.repo_dir_name());
        assert!(a.repo_dir_name().starts_with("b-"));
    }
}
