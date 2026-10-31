/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! The dependency graph a lockfile pins, resolved the way Cargo resolves it: every
//! requirement picks the highest non-yanked version it allows, features flow along the
//! edges, and an optional dependency enters the graph only when a feature turns it on.
//! Requirements that overlap one compatibility range share one version; incompatible
//! ones get their own. A patched package replaces its registry version.

use anyhow::{bail, Context, Result};
use semver::{Version, VersionReq};
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};

use super::git_resolve::GitResolver;
use super::lockfile::{CargoLock, CargoLockPackage};
use super::registry_config::index_to_lock_source;
use super::resolver::{ManifestDirectDep, NativeResolver};
use super::sparse_index::{SparseIndexDep, SparseIndexEntry};

type Key = (String, String);

struct Node {
    name: String,
    version: String,
    source: Option<String>,
    checksum: String,
    deps: Vec<SparseIndexDep>,
    features: HashMap<String, Vec<String>>,
    /// Optional dependencies without a `dep:` reference have an implicit feature.
    implicit: HashSet<String>,
    enabled: BTreeSet<String>,
    activated: HashSet<String>,
    requested: HashSet<String>,
    weak_pending: HashMap<String, Vec<String>>,
    dependencies: BTreeSet<String>,
}

impl Node {
    fn from_entry(entry: &SparseIndexEntry, source: &str) -> Self {
        let deps: Vec<SparseIndexDep> = entry
            .deps
            .iter()
            .filter(|d| d.kind.as_deref() != Some("dev"))
            .cloned()
            .collect();
        let explicit: HashSet<String> = entry
            .features
            .values()
            .flatten()
            .filter_map(|v| v.strip_prefix("dep:").map(str::to_string))
            .collect();
        let implicit = deps
            .iter()
            .filter(|d| d.optional && !explicit.contains(&d.name))
            .map(|d| d.name.clone())
            .collect();
        Self {
            name: entry.name.clone(),
            version: entry.vers.clone(),
            source: Some(source.to_string()),
            checksum: entry.cksum.clone(),
            deps,
            features: entry.features.clone(),
            implicit,
            enabled: BTreeSet::new(),
            activated: HashSet::new(),
            requested: HashSet::new(),
            weak_pending: HashMap::new(),
            dependencies: BTreeSet::new(),
        }
    }

    /// A git, path or patched package whose dependencies were already handed to the queue.
    fn opaque(pkg: &CargoLockPackage) -> Self {
        Self {
            name: pkg.name.clone(),
            version: pkg.version.clone(),
            source: pkg.source.clone(),
            checksum: pkg.checksum.clone().unwrap_or_default(),
            deps: Vec::new(),
            features: HashMap::new(),
            implicit: HashSet::new(),
            enabled: BTreeSet::new(),
            activated: HashSet::new(),
            requested: HashSet::new(),
            weak_pending: HashMap::new(),
            dependencies: pkg.dependencies.iter().cloned().collect(),
        }
    }

    fn is_optional(&self, dep: &str) -> bool {
        self.deps.iter().any(|d| d.name == dep && d.optional)
    }

    fn has_dep(&self, dep: &str) -> bool {
        self.deps.iter().any(|d| d.name == dep)
    }

    /// Enables features, closing over their definitions; returns features newly requested
    /// on dependencies (by dependency name).
    fn enable(&mut self, feats: &[String]) -> HashMap<String, Vec<String>> {
        let mut dep_reqs: HashMap<String, Vec<String>> = HashMap::new();
        let mut stack: Vec<String> = feats.to_vec();
        while let Some(f) = stack.pop() {
            if let Some(vals) = self.features.get(&f).cloned() {
                if !self.enabled.insert(f) {
                    continue;
                }
                for v in vals {
                    if let Some(d) = v.strip_prefix("dep:") {
                        self.activate(d, &mut dep_reqs);
                    } else if let Some((d, df)) = v.split_once('/') {
                        let (d, weak) = match d.strip_suffix('?') {
                            Some(x) => (x, true),
                            None => (d, false),
                        };
                        if !weak {
                            self.activate(d, &mut dep_reqs);
                            dep_reqs
                                .entry(d.to_string())
                                .or_default()
                                .push(df.to_string());
                        } else if self.activated.contains(d)
                            || (self.has_dep(d) && !self.is_optional(d))
                        {
                            dep_reqs
                                .entry(d.to_string())
                                .or_default()
                                .push(df.to_string());
                        } else {
                            self.weak_pending
                                .entry(d.to_string())
                                .or_default()
                                .push(df.to_string());
                        }
                    } else {
                        stack.push(v);
                    }
                }
            } else if self.implicit.contains(&f) && self.enabled.insert(f.clone()) {
                self.activate(&f, &mut dep_reqs);
            }
            // An unknown feature on a dependency is ignored, as Cargo ignores it there.
        }
        dep_reqs
    }

    fn activate(&mut self, dep: &str, dep_reqs: &mut HashMap<String, Vec<String>>) {
        if self.is_optional(dep) && self.activated.insert(dep.to_string()) {
            if let Some(pending) = self.weak_pending.remove(dep) {
                dep_reqs.entry(dep.to_string()).or_default().extend(pending);
            }
        }
    }
}

fn req_of(req: &str) -> VersionReq {
    let clean = req.trim().trim_matches('"');
    if clean.is_empty() || clean == "*" {
        VersionReq::STAR
    } else {
        VersionReq::parse(clean).unwrap_or(VersionReq::STAR)
    }
}

/// Cargo's rule for versions that must unify: same major, or same minor below 1.0.
fn same_compat_range(a: &str, b: &str) -> bool {
    match (Version::parse(a), Version::parse(b)) {
        (Ok(a), Ok(b)) => {
            if a.major > 0 || b.major > 0 {
                a.major == b.major
            } else if a.minor > 0 || b.minor > 0 {
                a.minor == b.minor
            } else {
                a.patch == b.patch
            }
        }
        _ => false,
    }
}

fn pick_version<'a>(
    entries: &'a [SparseIndexEntry],
    reqs: &[String],
) -> Option<&'a SparseIndexEntry> {
    let parsed: Vec<VersionReq> = reqs.iter().map(|r| req_of(r)).collect();
    let mut best: Option<(&SparseIndexEntry, Version)> = None;
    for e in entries.iter().filter(|e| !e.yanked) {
        let Ok(v) = Version::parse(&e.vers) else {
            continue;
        };
        if parsed.iter().all(|r| r.matches(&v)) && best.as_ref().is_none_or(|(_, bv)| v > *bv) {
            best = Some((e, v));
        }
    }
    best.map(|(e, _)| e)
}

impl NativeResolver {
    pub(crate) fn resolve_graph(
        &self,
        roots: &[ManifestDirectDep],
        offline: bool,
    ) -> Result<CargoLock> {
        // Requirements learnt from conflicts; a conflict restarts the resolution with them.
        let mut pins: HashMap<Key, Vec<String>> = HashMap::new();
        let mut index_cache: HashMap<Key, Vec<SparseIndexEntry>> = HashMap::new();
        for _ in 0..64 {
            match self.resolve_once(roots, offline, &pins, &mut index_cache)? {
                Ok(lock) => return Ok(lock),
                Err((key, req)) => pins.entry(key).or_default().push(req),
            }
        }
        bail!("dependency resolution did not converge after 64 attempts")
    }

    /// `Err((key, req))` asks for a restart with `req` pinned on `key`.
    #[allow(clippy::type_complexity)]
    fn resolve_once(
        &self,
        roots: &[ManifestDirectDep],
        offline: bool,
        pins: &HashMap<Key, Vec<String>>,
        index_cache: &mut HashMap<Key, Vec<SparseIndexEntry>>,
    ) -> Result<std::result::Result<CargoLock, (Key, String)>> {
        let patches = self.patches.lock().unwrap().clone();
        let config = self.config.lock().unwrap().clone();
        let mut git = GitResolver::for_cache_dir(self.cache_dir.as_deref(), config.clone());
        let mut nodes: Vec<Node> = Vec::new();
        let mut by_key: HashMap<Key, Vec<usize>> = HashMap::new();
        let mut extra: Vec<CargoLockPackage> = Vec::new();
        let mut requested_by: HashMap<Key, String> = HashMap::new();
        let mut seen: HashSet<(Key, String, Vec<String>, bool)> = HashSet::new();
        let mut queue: VecDeque<ManifestDirectDep> = roots.iter().cloned().collect();
        let key = |r: &ManifestDirectDep| -> Key { (r.source.clone(), r.name.clone()) };

        while !queue.is_empty() {
            let wave: Vec<ManifestDirectDep> = queue
                .drain(..)
                .filter(|r| {
                    let mut f = r.features.clone();
                    f.sort();
                    f.dedup();
                    seen.insert((key(r), r.req.clone(), f, r.default_features))
                })
                .collect();
            let to_fetch: Vec<Key> = wave
                .iter()
                .map(key)
                .filter(|k| !index_cache.contains_key(k) && patches.get(&k.0, &k.1).is_none())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            self.fetch_entries(&to_fetch, offline, index_cache)?;

            for r in wave {
                let k = key(&r);
                let existing = by_key.get(&k).and_then(|idxs| {
                    idxs.iter()
                        .copied()
                        .find(|&i| {
                            Version::parse(&nodes[i].version)
                                .is_ok_and(|v| req_of(&r.req).matches(&v))
                        })
                        .map(Ok)
                        .or_else(|| {
                            idxs.iter()
                                .copied()
                                .find(|&i| {
                                    same_compat_range(&nodes[i].version, &lowest_bound(&r.req))
                                })
                                .map(Err)
                        })
                });
                let idx = match existing {
                    Some(Ok(i)) => i,
                    Some(Err(conflicting)) => {
                        // Same compatibility range, no shared version yet: restart with both
                        // requirements pinned, as a unifying resolver would.
                        let _ = conflicting;
                        return Ok(Err((k, r.req.clone())));
                    }
                    None => {
                        let node = if let Some((base, dep)) = patches.get(&r.source, &r.name) {
                            let res = super::patch_resolve::resolve_patch(
                                &mut git, &config, &r.name, base, dep, offline,
                            )?;
                            let primary =
                                res.primary.expect("patch resolution has a primary package");
                            if !Version::parse(&primary.version)
                                .is_ok_and(|v| req_of(&r.req).matches(&v))
                            {
                                bail!(
                                    "patch for `{}` has version {}, which does not satisfy the requirement `{}`",
                                    r.name, primary.version, r.req
                                );
                            }
                            queue.extend(res.registry_deps);
                            extra.extend(res.packages);
                            Node::opaque(&primary)
                        } else {
                            let entries = &index_cache[&k];
                            let mut reqs = vec![r.req.clone()];
                            reqs.extend(pins.get(&k).cloned().unwrap_or_default());
                            let chosen = pick_version(entries, &reqs).with_context(|| {
                                let by = requested_by
                                    .get(&k)
                                    .map(|p| format!(" (required by {p})"))
                                    .unwrap_or_default();
                                format!(
                                    "resolving {} {}{by} from {}: no non-yanked version satisfies {}",
                                    r.name, r.req, r.source, reqs.join(" and ")
                                )
                            })?;
                            Node::from_entry(chosen, &r.source)
                        };
                        nodes.push(node);
                        let i = nodes.len() - 1;
                        by_key.entry(k.clone()).or_default().push(i);
                        i
                    }
                };

                let mut feats = r.features.clone();
                if r.default_features && nodes[idx].features.contains_key("default") {
                    feats.push("default".to_string());
                }
                let mut dep_feats = nodes[idx].enable(&feats);
                let parent = format!("{} {}", nodes[idx].name, nodes[idx].version);
                let parent_source = nodes[idx]
                    .source
                    .clone()
                    .unwrap_or_else(|| r.source.clone());
                for d in nodes[idx].deps.clone() {
                    if d.optional && !nodes[idx].activated.contains(&d.name) {
                        continue;
                    }
                    let more = dep_feats.remove(&d.name).unwrap_or_default();
                    let first = nodes[idx].requested.insert(d.name.clone());
                    if !first && more.is_empty() {
                        continue;
                    }
                    let pkg_name = d.package.clone().unwrap_or_else(|| d.name.clone());
                    let source = d
                        .registry
                        .as_deref()
                        .map(index_to_lock_source)
                        .unwrap_or_else(|| parent_source.clone());
                    nodes[idx].dependencies.insert(pkg_name.clone());
                    requested_by
                        .entry((source.clone(), pkg_name.clone()))
                        .or_insert_with(|| parent.clone());
                    let mut features = d.features.clone();
                    features.extend(more);
                    queue.push_back(ManifestDirectDep {
                        name: pkg_name,
                        req: d.req.clone(),
                        source,
                        features,
                        default_features: d.default_features,
                    });
                }
            }
        }

        // Cargo writes "name version" for a dependency whose name several packages of the
        // lock share (tonic: strum 0.27 and 0.28); a bare name would be ambiguous when the
        // lock is read back. The dependent's own requirement picks the version.
        let mut versions_by_name: HashMap<String, Vec<String>> = HashMap::new();
        for n in &nodes {
            versions_by_name
                .entry(n.name.clone())
                .or_default()
                .push(n.version.clone());
        }
        for p in &extra {
            versions_by_name
                .entry(p.name.clone())
                .or_default()
                .push(p.version.clone());
        }
        let qualify = |n: &Node, dep: &str| -> String {
            if dep.contains(' ') {
                return dep.to_string();
            }
            let Some(versions) = versions_by_name.get(dep).filter(|v| v.len() > 1) else {
                return dep.to_string();
            };
            let req = n
                .deps
                .iter()
                .find(|d| d.package.as_deref().unwrap_or(&d.name) == dep)
                .and_then(|d| semver::VersionReq::parse(&d.req).ok());
            let Some(req) = req else {
                return dep.to_string();
            };
            let chosen = versions
                .iter()
                .filter_map(|v| semver::Version::parse(v).ok().filter(|pv| req.matches(pv)))
                .max();
            match chosen {
                Some(v) => format!("{dep} {v}"),
                None => dep.to_string(),
            }
        };
        let mut packages: Vec<CargoLockPackage> = nodes
            .iter()
            .map(|n| CargoLockPackage {
                name: n.name.clone(),
                version: n.version.clone(),
                source: n.source.clone(),
                checksum: (!n.checksum.is_empty()).then_some(n.checksum.clone()),
                dependencies: n.dependencies.iter().map(|d| qualify(n, d)).collect(),
            })
            .collect();
        for p in extra {
            if !packages
                .iter()
                .any(|q| q.name == p.name && q.version == p.version && q.source == p.source)
            {
                packages.push(p);
            }
        }
        packages.sort_by(|a, b| a.name.cmp(&b.name).then(a.version.cmp(&b.version)));
        Ok(Ok(CargoLock {
            version: 3,
            packages,
        }))
    }

    /// Fetches the index entries of `keys`, in parallel when there are several.
    fn fetch_entries(
        &self,
        keys: &[Key],
        offline: bool,
        index_cache: &mut HashMap<Key, Vec<SparseIndexEntry>>,
    ) -> Result<()> {
        if keys.is_empty() {
            return Ok(());
        }
        if keys.len() == 1 {
            let k = &keys[0];
            let entries = self.source_for(&k.0)?.versions(&k.1, offline)?;
            index_cache.insert(k.clone(), entries);
            return Ok(());
        }
        let n = keys.len();
        let cursor = std::sync::atomic::AtomicUsize::new(0);
        let results: Vec<std::sync::Mutex<Option<Result<Vec<SparseIndexEntry>>>>> =
            (0..n).map(|_| std::sync::Mutex::new(None)).collect();
        std::thread::scope(|s| {
            for _ in 0..n.min(32) {
                s.spawn(|| loop {
                    let i = cursor.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if i >= n {
                        break;
                    }
                    let res = self
                        .source_for(&keys[i].0)
                        .and_then(|src| src.versions(&keys[i].1, offline));
                    *results[i].lock().unwrap() = Some(res);
                });
            }
        });
        for (k, slot) in keys.iter().zip(results) {
            index_cache.insert(k.clone(), slot.into_inner().unwrap().unwrap()?);
        }
        Ok(())
    }
}

/// The lowest version a requirement can accept, to tell which compatibility range it is in.
fn lowest_bound(req: &str) -> String {
    let clean = req.trim().trim_matches('"');
    let first = clean.split(',').next().unwrap_or("").trim();
    let digits = first.trim_start_matches(['^', '~', '=', '>', '<', ' ']);
    let mut parts: Vec<&str> = digits.split('.').collect();
    while parts.len() < 3 {
        parts.push("0");
    }
    parts[..3]
        .iter()
        .map(|p| if p.is_empty() || *p == "*" { "0" } else { p })
        .collect::<Vec<_>>()
        .join(".")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compat_ranges_follow_cargo() {
        assert!(same_compat_range("1.2.0", "1.9.9"));
        assert!(!same_compat_range("1.2.0", "2.0.0"));
        assert!(same_compat_range("0.7.1", "0.7.3"));
        assert!(!same_compat_range("0.7.1", "0.8.0"));
        assert_eq!(lowest_bound("^1.5"), "1.5.0");
        assert_eq!(lowest_bound(">=0.7, <0.9"), "0.7.0");
        assert_eq!(lowest_bound("*"), "0.0.0");
    }
}
