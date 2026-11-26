/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! The project's own crates are built into its target directory, not into the shared cache.
//!
//! Workspace members and path dependencies change with every edit: each build of them is a
//! new version that no other build will ask for again (hitmeup: 70 GB of its own test
//! executables in the shared store within two hours, one version per change and worktree).
//! So their outputs, build-script results and cache entries live in a store of the target
//! directory (`<target>/<profile>/.mitts-store`), which keeps one version of each: after a
//! build, the versions it replaced are removed. A build that changes nothing restores from
//! it as before, and `mitts clean` removes it with the target. The shared cache holds the
//! registry and git dependencies (and their build scripts) only.
//!
//! `MITTS_SHARE_WORKSPACE_CRATES=1` stores them in the shared cache as before: a second
//! worktree of the same commit then reuses them instead of building them again.

use super::types::PlannedCompilationUnit;
use crate::core::action_cache::{ActionCache, ActionCacheEntry};
use crate::core::cas::ContentAddressedStore;
use crate::core::lock::FileLock;
use anyhow::Result;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Set to `1` to keep the project's own crates in the shared cache.
pub const SHARE_VAR: &str = "MITTS_SHARE_WORKSPACE_CRATES";
/// The store's directory in a profile directory of the target.
pub const DIR: &str = ".mitts-store";
/// Build-script results and manifests the project has not used for this long are removed.
const UNUSED_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 3600);

/// Whether the project's own crates go to the shared cache (`MITTS_SHARE_WORKSPACE_CRATES`).
pub fn shares_workspace_crates() -> bool {
    std::env::var(SHARE_VAR).is_ok_and(|v| matches!(v.trim(), "1" | "true" | "yes"))
}

/// Whether `unit` (a workspace member or a path dependency) is kept in the project's store.
pub fn keeps_local(unit: &PlannedCompilationUnit) -> bool {
    super::unit_inputs::is_mutable_unit(unit) && !shares_workspace_crates()
}

/// The store of one profile directory of a target.
pub struct MemberStore {
    pub cas: ContentAddressedStore,
    pub ac: ActionCache,
    root: PathBuf,
    /// Held while a build uses the store; pruning needs it alone.
    lease: Option<FileLock>,
}

impl MemberStore {
    pub fn open(target_dir: &Path, profile: &str) -> Result<Self> {
        let root = target_dir.join(profile).join(DIR);
        let cas = ContentAddressedStore::new(root.join("cas"))?;
        let ac = ActionCache::new(root.join("ac"))?;
        let lease = FileLock::acquire_shared(&root.join(".lock")).ok();
        Ok(Self {
            cas,
            ac,
            root,
            lease,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// After a build in `mode` (build, test, check...): removes the versions this build
    /// replaced. What the last build of each other mode used stays: `mitts test` compiles
    /// the crates under other keys than `mitts build`, and switching between the two must
    /// not rebuild them each time. Skipped while another build uses the store (its next
    /// build prunes). The bytes freed.
    pub fn prune_after_build(mut self, mode: &str) -> u64 {
        let used = self.ac.used_keys();
        drop(self.lease.take());
        let Ok(Some(_alone)) = FileLock::try_acquire_exclusive(&self.root.join(".lock")) else {
            return 0;
        };
        let mode: String = mode.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
        let mut lines: Vec<&str> = used.iter().map(String::as_str).collect();
        lines.sort_unstable();
        let _ = fs::write(
            self.root.join(format!("used-{mode}")),
            lines.join("\n") + "\n",
        );
        let mut keep = used.clone();
        for path in fs::read_dir(&self.root)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
        {
            if path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("used-"))
            {
                let text = fs::read_to_string(&path).unwrap_or_default();
                keep.extend(text.lines().filter(|l| !l.is_empty()).map(String::from));
            }
        }
        let (freed, replaced) = prune(&self.cas, &self.ac, &used, &keep);
        freed + self.remove_replaced_outputs(replaced)
    }

    /// The files the replaced versions left in the profile directory. Their names carry the
    /// version (`libcore-<hash>.rlib`), so each edit would leave one more beside the new one:
    /// unlike Cargo, whose names do not change with the sources. A file is removed once no
    /// remaining entry names it.
    fn remove_replaced_outputs(&self, replaced: Vec<PathBuf>) -> u64 {
        let Some(profile_dir) = self.root.parent() else {
            return 0;
        };
        let Some(target_dir) = profile_dir.parent() else {
            return 0;
        };
        let place = |rel: &Path| -> PathBuf {
            if rel.components().count() > 1 {
                target_dir.join(rel)
            } else {
                profile_dir.join("deps").join(rel)
            }
        };
        let kept: HashSet<PathBuf> = read_entries(&self.ac)
            .iter()
            .flat_map(|(_, e)| e.outputs.iter().map(|o| place(&o.relative_path)))
            .collect();
        let mut freed = 0;
        for rel in replaced {
            let path = place(&rel);
            if kept.contains(&path) {
                continue;
            }
            let size = fs::metadata(&path).map_or(0, |m| m.len());
            if remove_file(&path) {
                freed += size;
            }
        }
        freed
    }
}

/// `name` without the `-<16 hex digits>` metadata suffix rustc's `-C extra-filename` puts
/// before the extension (or at the end), which changes with every version.
fn without_version(name: &str) -> String {
    let (stem, ext) = match name.find('.') {
        Some(dot) => name.split_at(dot),
        None => (name, ""),
    };
    let unversioned = stem
        .rsplit_once('-')
        .filter(|(_, tail)| tail.len() == 16 && tail.bytes().all(|b| b.is_ascii_hexdigit()))
        .map_or(stem, |(head, _)| head);
    format!("{unversioned}{ext}")
}

/// What an entry produces: its output paths without their version suffix. A check
/// (`.rmeta`) and a build (`.rlib`, `.rmeta`) of one library, or a binary
/// (`debug/app`) and its unit tests (`debug/deps/app-<hash>`), are different targets of
/// one unit and both stay.
fn shape(entry: &ActionCacheEntry) -> BTreeSet<String> {
    entry
        .outputs
        .iter()
        .map(|o| {
            let parent = o.relative_path.parent().unwrap_or(Path::new(""));
            let name = o
                .relative_path
                .file_name()
                .map(|n| without_version(&n.to_string_lossy()))
                .unwrap_or_default();
            format!("{}/{name}", parent.to_string_lossy())
        })
        .collect()
}

fn read_entries(ac: &ActionCache) -> Vec<(PathBuf, ActionCacheEntry)> {
    fs::read_dir(ac.root())
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .filter_map(|p| {
            let entry = serde_json::from_str(&fs::read_to_string(&p).ok()?).ok()?;
            Some((p, entry))
        })
        .collect()
}

fn remove_file(path: &Path) -> bool {
    #[cfg(not(unix))]
    if let Ok(meta) = fs::metadata(path) {
        let mut perms = meta.permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        let _ = fs::set_permissions(path, perms);
    }
    fs::remove_file(path).is_ok()
}

fn unused_for(path: &Path, now: SystemTime) -> bool {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| now.duration_since(t).ok())
        .is_some_and(|age| age > UNUSED_MAX_AGE)
}

/// Removes every entry whose unit this build (`used`) built or restored in another version
/// (same unit, same [`shape`]) unless it is in `keep` (what the last build of each mode
/// used), then the objects and named artifacts no remaining entry uses. The bytes freed and
/// the outputs (relative paths) of the removed entries.
pub fn prune(
    cas: &ContentAddressedStore,
    ac: &ActionCache,
    used: &HashSet<String>,
    keep: &HashSet<String>,
) -> (u64, Vec<PathBuf>) {
    let entries = read_entries(ac);
    let mut groups: HashMap<(&str, BTreeSet<String>), Vec<usize>> = HashMap::new();
    for (i, (_, e)) in entries.iter().enumerate() {
        groups
            .entry((e.action_id.as_str(), shape(e)))
            .or_default()
            .push(i);
    }
    let mut removed = vec![false; entries.len()];
    for members in groups.values() {
        if members
            .iter()
            .any(|&i| used.contains(&entries[i].1.action_key))
        {
            for &i in members {
                if !keep.contains(&entries[i].1.action_key) {
                    removed[i] = fs::remove_file(&entries[i].0).is_ok();
                }
            }
        }
    }
    let replaced: Vec<PathBuf> = entries
        .iter()
        .zip(&removed)
        .filter(|(_, gone)| **gone)
        .flat_map(|((_, e), _)| e.outputs.iter().map(|o| o.relative_path.clone()))
        .collect();
    let kept: HashSet<&str> = entries
        .iter()
        .zip(&removed)
        .filter(|(_, gone)| !**gone)
        .flat_map(|((_, e), _)| e.outputs.iter().map(|o| o.cas_hash.as_str()))
        .collect();

    let mut freed = 0;
    for path in fs::read_dir(cas.root().join("objects"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
    {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if kept.contains(name) {
            continue;
        }
        let size = fs::metadata(&path).map_or(0, |m| m.len());
        if remove_file(&path) {
            freed += size;
        }
    }
    for dir in ["artifacts", "artifacts-meta"].map(|d| cas.root().join(d)) {
        for path in fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
        {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.starts_with('.') {
                continue;
            }
            let sidecar = dir.join(format!(".{name}.cas_hash"));
            let orphaned = match fs::read_to_string(&sidecar) {
                Ok(hash) => !kept.contains(hash.trim()),
                // Without a sidecar the name is a link to its object: alone, it is orphaned.
                Err(_) => crate::core::file_id::of(&path).is_some_and(|id| id.nlink <= 1),
            };
            if orphaned && remove_file(&path) {
                let _ = remove_file(&sidecar);
            }
        }
    }

    // Manifests and build-script results are small; those unused for a week go.
    let now = SystemTime::now();
    for path in fs::read_dir(ac.manifest_dir())
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
    {
        if unused_for(&path, now) {
            let _ = fs::remove_file(&path);
        }
    }
    for base in fs::read_dir(cas.root().join("build_scripts"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
    {
        let runs: Vec<PathBuf> = fs::read_dir(&base)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .collect();
        let all_unused = runs.iter().all(|run| {
            let last_used = run.join(".last_used");
            unused_for(if last_used.exists() { &last_used } else { run }, now)
        });
        if all_unused {
            let _ = super::build_script_cas::remove_dir_all_readonly_safe(&base);
        }
    }
    (freed, replaced)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::action_cache::CachedOutput;

    fn entry(key: &str, id: &str, out: &str, hash: &str, executable: bool) -> ActionCacheEntry {
        ActionCacheEntry {
            action_key: key.to_string(),
            action_id: id.to_string(),
            created_at_secs: 0,
            outputs: vec![CachedOutput {
                relative_path: PathBuf::from(out),
                cas_hash: hash.to_string(),
                file_size: 1,
                executable,
            }],
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            target_scopes: Vec::new(),
        }
    }

    #[test]
    fn the_version_suffix_is_what_differs_between_versions() {
        assert_eq!(
            without_version("libapp-0123456789abcdef.rlib"),
            "libapp.rlib"
        );
        assert_eq!(without_version("app-0123456789abcdef"), "app");
        assert_eq!(without_version("app-0123456789abcdef.exe"), "app.exe");
        assert_eq!(without_version("app"), "app");
        assert_eq!(without_version("my-app.pdb"), "my-app.pdb");
    }

    #[test]
    fn a_build_keeps_the_versions_it_used_and_drops_the_ones_they_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let cas = ContentAddressedStore::new(dir.path().join("cas")).unwrap();
        let ac = ActionCache::new(dir.path().join("ac")).unwrap();
        let mut hashes = HashMap::new();
        for name in [
            "old-lib", "new-lib", "old-test", "new-test", "check", "other",
        ] {
            let src = dir.path().join(name);
            fs::write(&src, name).unwrap();
            hashes.insert(name, cas.store_file(&src).unwrap().0);
        }
        let h = |n: &str| hashes[n].as_str();
        let lib = "app@0.1.0::lib:app";
        let (v1, v2, v3) = ("0123456789abcdef", "fedcba9876543210", "00000000ffffffff");
        let entries = [
            entry(
                "k-old-lib",
                lib,
                &format!("libapp-{v1}.rlib"),
                h("old-lib"),
                false,
            ),
            entry(
                "k-new-lib",
                lib,
                &format!("libapp-{v2}.rlib"),
                h("new-lib"),
                false,
            ),
            // The library's unit tests: same unit, another target.
            entry(
                "k-old-test",
                lib,
                &format!("debug/deps/app-{v1}"),
                h("old-test"),
                true,
            ),
            entry(
                "k-new-test",
                lib,
                &format!("debug/deps/app-{v2}"),
                h("new-test"),
                true,
            ),
            // A check of the library, not built this time: stays.
            entry(
                "k-check",
                lib,
                &format!("libapp-{v3}.rmeta"),
                h("check"),
                false,
            ),
            // A unit this build did not touch at all: stays.
            entry(
                "k-other",
                "b@0.1.0::lib:b",
                &format!("libb-{v1}.rlib"),
                h("other"),
                false,
            ),
        ];
        for e in &entries {
            ac.put(e).unwrap();
        }
        let used: HashSet<String> = ["k-new-lib", "k-new-test"].map(String::from).into();
        prune(&cas, &ac, &used, &used);

        for (key, hash, stays) in [
            ("k-old-lib", "old-lib", false),
            ("k-new-lib", "new-lib", true),
            ("k-old-test", "old-test", false),
            ("k-new-test", "new-test", true),
            ("k-check", "check", true),
            ("k-other", "other", true),
        ] {
            assert_eq!(ac.entry_path(key).is_file(), stays, "{key}");
            assert_eq!(cas.has_object(h(hash)), stays, "{hash}");
        }
    }
}
