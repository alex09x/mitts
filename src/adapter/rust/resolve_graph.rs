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

use anyhow::Result;
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
    /// `(package, requirement)` per edge: one package may depend on two versions of a
    /// crate under two names (serde_with: `schemars` 0.9 and 1), and each must stay.
    dependencies: BTreeSet<(String, String)>,
    /// A git, path or patched package: no index feature table to check against.
    opaque: bool,
}

impl Node {
    fn from_entry(entry: &SparseIndexEntry, source: &str) -> Self {
        let deps: Vec<SparseIndexDep> = entry
            .deps
            .iter()
            .filter(|d| d.kind.as_deref() != Some("dev"))
            .cloned()
            .collect();
        let features = all_features(entry);
        let explicit: HashSet<String> = features
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
            features,
            implicit,
            enabled: BTreeSet::new(),
            activated: HashSet::new(),
            requested: HashSet::new(),
            dependencies: BTreeSet::new(),
            opaque: false,
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
            // Already written as lock entries ("name" or "name version").
            dependencies: pkg
                .dependencies
                .iter()
                .map(|d| (d.clone(), String::new()))
                .collect(),
            opaque: true,
        }
    }

    fn is_optional(&self, dep: &str) -> bool {
        self.deps.iter().any(|d| d.name == dep && d.optional)
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
                        self.activate(d);
                    } else if let Some((d, df)) = v.split_once('/') {
                        // `dep?/feat` is weak: a build leaves an inactive optional dependency
                        // out. Cargo's resolver, and so Cargo.lock, still includes it with the
                        // requested feature; a lockfile that omitted it is rejected by
                        // `cargo --locked`. Whether the build uses it is decided by the
                        // feature graph, not here.
                        let d = d.strip_suffix('?').unwrap_or(d);
                        self.activate(d);
                        dep_reqs
                            .entry(d.to_string())
                            .or_default()
                            .push(df.to_string());
                    } else {
                        stack.push(v);
                    }
                }
            } else if self.implicit.contains(&f) && self.enabled.insert(f.clone()) {
                self.activate(&f);
            }
            // An unknown feature on a dependency is ignored, as Cargo ignores it there.
        }
        dep_reqs
    }

    fn activate(&mut self, dep: &str) {
        if self.is_optional(dep) {
            self.activated.insert(dep.to_string());
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

/// The compatibility range a version belongs to, as a pin key: `2` for 2.x, `0.9` for
/// 0.9.x, `0.0.3` for 0.0.3. Two versions of one crate in different ranges are separate
/// nodes and must not share what a conflict taught about one of them.
fn compat_range(version: &str) -> String {
    match Version::parse(version) {
        Ok(v) if v.major > 0 => v.major.to_string(),
        Ok(v) if v.minor > 0 => format!("0.{}", v.minor),
        Ok(v) => format!("0.0.{}", v.patch),
        Err(_) => version.to_string(),
    }
}

/// What a restart learnt about one crate in one compatibility range.
#[derive(Default, Clone)]
struct Pin {
    reqs: Vec<String>,
    features: Vec<String>,
}

type PinKey = (Key, String);

/// Restart with these added to the pin of `key`.
struct Restart {
    key: PinKey,
    req: String,
    features: Vec<String>,
}

/// Whether an already chosen node offers `wanted`. A git, path or patched node knows no
/// feature table here and is taken as it is.
fn node_offers(node: &Node, wanted: &[String]) -> bool {
    if node.opaque {
        return true;
    }
    wanted
        .iter()
        .all(|f| f == "default" || node.features.contains_key(f) || node.implicit.contains(f))
}

/// The feature table crates.io publishes in two parts: `features2` holds the ones that use
/// `dep:` or `?/` syntax, which older Cargo versions could not read.
fn all_features(entry: &SparseIndexEntry) -> HashMap<String, Vec<String>> {
    let mut features = entry.features.clone();
    for (name, values) in &entry.features2 {
        features
            .entry(name.clone())
            .or_default()
            .extend(values.iter().cloned());
    }
    features
}

/// Whether `entry` offers every feature in `wanted`: a declared feature, or the implicit
/// feature of an optional dependency that no `dep:` entry hides. Cargo skips a version
/// that lacks a requested feature (rsa 0.10.0 dropped `sha2`, which crypto-glue asks for).
fn has_features(entry: &SparseIndexEntry, wanted: &[String]) -> bool {
    if wanted.is_empty() {
        return true;
    }
    let features = all_features(entry);
    let hidden: HashSet<&str> = features
        .values()
        .flatten()
        .filter_map(|v| v.strip_prefix("dep:"))
        .collect();
    wanted.iter().all(|f| {
        f == "default"
            || features.contains_key(f)
            || entry.deps.iter().any(|d| {
                d.optional
                    && d.kind.as_deref() != Some("dev")
                    && d.name == *f
                    && !hidden.contains(f.as_str())
            })
    })
}

/// The version for `reqs`: a version the existing lock pins (`preferred`) when one still
/// satisfies them, even if yanked since, as Cargo keeps it; otherwise the highest
/// non-yanked one. Either way it must offer the requested `features`. With a Rust
/// `limit` (`incompatible-rust-versions = "fallback"`), a fresh pick prefers the highest
/// version whose `rust-version` fits it, and takes the highest overall when none does.
fn pick_version<'a>(
    entries: &'a [SparseIndexEntry],
    reqs: &[String],
    features: &[String],
    preferred: &[String],
    limit: Option<(u64, u64, u64)>,
) -> Option<&'a SparseIndexEntry> {
    if let Some(limit) = limit {
        let supported: Vec<SparseIndexEntry> = entries
            .iter()
            .filter(|e| {
                preferred.contains(&e.vers)
                    || super::rust_version::supported(e.rust_version.as_deref(), limit)
            })
            .cloned()
            .collect();
        if let Some(found) = pick_version(&supported, reqs, features, preferred, None) {
            return entries.iter().find(|e| e.vers == found.vers);
        }
    }
    let parsed: Vec<VersionReq> = reqs.iter().map(|r| req_of(r)).collect();
    let fits = |e: &SparseIndexEntry, v: &Version| {
        parsed.iter().all(|r| r.matches(v)) && has_features(e, features)
    };
    let mut best: Option<(&SparseIndexEntry, Version)> = None;
    for e in entries.iter().filter(|e| preferred.contains(&e.vers)) {
        let Ok(v) = Version::parse(&e.vers) else {
            continue;
        };
        if fits(e, &v) && best.as_ref().is_none_or(|(_, bv)| v > *bv) {
            best = Some((e, v));
        }
    }
    if best.is_some() {
        return best.map(|(e, _)| e);
    }
    for e in entries.iter().filter(|e| !e.yanked) {
        let Ok(v) = Version::parse(&e.vers) else {
            continue;
        };
        if fits(e, &v) && best.as_ref().is_none_or(|(_, bv)| v > *bv) {
            best = Some((e, v));
        }
    }
    best.map(|(e, _)| e)
}

/// The graph has no solution under the current choices (no version satisfies a request,
/// or conflicts did not settle) — as opposed to an index, network or configuration error.
#[derive(Debug)]
pub(crate) struct Unsatisfiable {
    message: String,
    /// The registry package version whose dependency could not be satisfied: resolving
    /// again without that version is how the resolver backtracks.
    blame: Vec<(Key, String)>,
}

impl Unsatisfiable {
    pub(crate) fn new(message: String) -> Self {
        Unsatisfiable {
            message,
            blame: Vec::new(),
        }
    }
}

impl std::fmt::Display for Unsatisfiable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// Resolution attempts one `generate-lockfile` may spend across all backtracking branches.
const RESOLVE_ATTEMPTS: usize = 1024;

/// The search ran out of attempts before it settled: an error of its own, never
/// `Unsatisfiable`, since unexplored choices may still resolve.
#[derive(Debug)]
pub(crate) struct SearchBudgetExhausted;

impl std::fmt::Display for SearchBudgetExhausted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "dependency resolution did not finish within {RESOLVE_ATTEMPTS} attempts; the graph \
             may still have a solution"
        )
    }
}

impl std::error::Error for SearchBudgetExhausted {}

fn search_budget_exhausted() -> anyhow::Error {
    anyhow::Error::new(SearchBudgetExhausted)
}

/// A backtracking branch's identity: its excluded versions, sorted.
fn branch_signature(excluded: &HashSet<(Key, String)>) -> Vec<String> {
    let mut signature: Vec<String> = excluded
        .iter()
        .map(|(k, v)| format!("{}\u{1f}{v}", key_string(k)))
        .collect();
    signature.sort();
    signature
}

/// The resolver key as one string, for the sets of packages the Rust limit is relaxed for.
fn key_string(k: &Key) -> String {
    format!("{}\u{1f}{}", k.0, k.1)
}

impl std::error::Error for Unsatisfiable {}

impl NativeResolver {
    pub(crate) fn resolve_graph(
        &self,
        roots: &[ManifestDirectDep],
        offline: bool,
    ) -> Result<CargoLock> {
        self.rust_affected.lock().unwrap().clear();
        self.set_relaxed(HashSet::new(), false);
        let result = self.resolve_graph_with_limit(roots, offline);
        // `fallback` is a preference. Only when the graph has no solution (not on a fetch or
        // configuration error) is the limit relaxed, and only for packages where it changes
        // the pick: one at a time, then all of them, then everywhere; a found graph is then
        // narrowed back to the fewest relaxed packages that still resolve.
        let limited = self.rust_limit.lock().unwrap().is_some();
        let unsatisfiable = result
            .as_ref()
            .err()
            .is_some_and(|e| e.downcast_ref::<Unsatisfiable>().is_some());
        if !limited || !unsatisfiable {
            return result;
        }
        let outcome = self.relax_until_resolved(roots, offline);
        self.set_relaxed(HashSet::new(), false);
        match outcome {
            Ok(Some(lock)) => Ok(lock),
            Ok(None) => result,
            Err(other) => Err(other),
        }
    }

    /// One attempt under the current relaxation: `Ok(None)` when the graph has no solution,
    /// any other error (fetch, configuration) passed on unchanged.
    fn try_relaxed(&self, roots: &[ManifestDirectDep], offline: bool) -> Result<Option<CargoLock>> {
        match self.resolve_graph_with_limit(roots, offline) {
            Ok(lock) => Ok(Some(lock)),
            Err(e) if e.downcast_ref::<Unsatisfiable>().is_some() => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn relax_until_resolved(
        &self,
        roots: &[ManifestDirectDep],
        offline: bool,
    ) -> Result<Option<CargoLock>> {
        let affected: Vec<String> = self.rust_affected.lock().unwrap().clone();
        for one in &affected {
            self.set_relaxed(HashSet::from([one.clone()]), false);
            if let Some(lock) = self.try_relaxed(roots, offline)? {
                return Ok(Some(lock));
            }
        }
        let mut relaxed: HashSet<String> = affected.iter().cloned().collect();
        self.set_relaxed(relaxed.clone(), false);
        let mut found = self.try_relaxed(roots, offline)?;
        if found.is_none() {
            // Everywhere; the run still records every decision the limit would change.
            self.set_relaxed(HashSet::new(), true);
            if self.try_relaxed(roots, offline)?.is_none() {
                return Ok(None);
            }
            relaxed = self.rust_affected.lock().unwrap().iter().cloned().collect();
            self.set_relaxed(relaxed.clone(), false);
            found = self.try_relaxed(roots, offline)?;
        }
        let Some(mut lock) = found else {
            return Ok(None);
        };
        // Narrow: every decision that still resolves under the limit goes back under it.
        let mut candidates: Vec<String> = relaxed.iter().cloned().collect();
        candidates.sort();
        for decision in candidates {
            let mut fewer = relaxed.clone();
            fewer.remove(&decision);
            self.set_relaxed(fewer.clone(), false);
            // No solution, or a trial that could not be evaluated (budget, an index entry
            // unavailable offline): narrowing is optional, the lock already found stands.
            if let Ok(Some(smaller)) = self.try_relaxed(roots, offline) {
                relaxed = fewer;
                lock = smaller;
            }
        }
        Ok(Some(lock))
    }

    fn set_relaxed(&self, keys: HashSet<String>, all: bool) {
        *self.rust_relaxed.lock().unwrap() = keys;
        *self.rust_relax_all.lock().unwrap() = all;
    }

    /// Backtracking search over version choices. A branch is a set of registry versions ruled
    /// out; when it has no solution, each version that asked for the failing request opens
    /// its own child branch with that version ruled out as well (those with another version
    /// to fall back to first). Branches are explored depth first, each set once, so a wrong
    /// guess in one branch never removes a version from another: the search ends at the
    /// first graph that resolves, or with the original conflict once every branch failed.
    fn resolve_graph_with_limit(
        &self,
        roots: &[ManifestDirectDep],
        offline: bool,
    ) -> Result<CargoLock> {
        let mut index_cache: HashMap<Key, Vec<SparseIndexEntry>> = HashMap::new();
        let mut budget = RESOLVE_ATTEMPTS;
        let mut stack: Vec<HashSet<(Key, String)>> = vec![HashSet::new()];
        let mut seen: HashSet<Vec<String>> = HashSet::new();
        let mut first_error: Option<anyhow::Error> = None;
        while let Some(excluded) = stack.pop() {
            if !seen.insert(branch_signature(&excluded)) {
                continue;
            }
            let error =
                match self.resolve_branch(roots, offline, &mut index_cache, &excluded, &mut budget)
                {
                    Ok(lock) => return Ok(lock),
                    Err(error) => error,
                };
            let Some(unsatisfiable) = error.downcast_ref::<Unsatisfiable>() else {
                return Err(error);
            };
            let mut open: Vec<(Key, String)> = Vec::new();
            for b in &unsatisfiable.blame {
                if !excluded.contains(b) && !open.contains(b) {
                    open.push(b.clone());
                }
            }
            let has_alternative = |(k, v): &(Key, String)| {
                index_cache.get(k).is_some_and(|entries| {
                    entries.iter().any(|e| {
                        !e.yanked
                            && e.vers != *v
                            && !excluded.contains(&(k.clone(), e.vers.clone()))
                    })
                })
            };
            // Stable: blame order within each group; pushed in reverse so the preferred
            // child is explored first.
            open.sort_by_key(|b| !has_alternative(b));
            for b in open.into_iter().rev() {
                let mut child = excluded.clone();
                child.insert(b);
                stack.push(child);
            }
            first_error.get_or_insert(error);
            // Only branches not explored yet count as left over.
            stack.retain(|s| !seen.contains(&branch_signature(s)));
            if budget == 0 && !stack.is_empty() {
                // Branches left unexplored: not a proof that the graph has no solution, so
                // not `Unsatisfiable` (which would relax the Rust limit).
                return Err(search_budget_exhausted());
            }
        }
        Err(first_error.unwrap_or_else(search_budget_exhausted))
    }

    /// One branch of the search: resolution restarted with the requirements and features
    /// learnt from conflicts (per crate and compatibility range) until it settles.
    fn resolve_branch(
        &self,
        roots: &[ManifestDirectDep],
        offline: bool,
        index_cache: &mut HashMap<Key, Vec<SparseIndexEntry>>,
        excluded: &HashSet<(Key, String)>,
        budget: &mut usize,
    ) -> Result<CargoLock> {
        let mut pins: HashMap<PinKey, Pin> = HashMap::new();
        loop {
            if *budget == 0 {
                return Err(search_budget_exhausted());
            }
            *budget -= 1;
            let restart = match self.resolve_once(roots, offline, &pins, index_cache, excluded)? {
                Ok(lock) => return Ok(lock),
                Err(restart) => restart,
            };
            let pin = pins.entry(restart.key).or_default();
            let mut learnt = false;
            if !pin.reqs.contains(&restart.req) {
                pin.reqs.push(restart.req);
                learnt = true;
            }
            for f in restart.features {
                if !pin.features.contains(&f) {
                    pin.features.push(f);
                    learnt = true;
                }
            }
            if !learnt {
                return Err(anyhow::Error::new(Unsatisfiable::new(
                    "dependency resolution did not converge: a conflict repeats".to_string(),
                )));
            }
        }
    }

    /// `Err(restart)` asks for a restart with more pinned on one crate.
    fn resolve_once(
        &self,
        roots: &[ManifestDirectDep],
        offline: bool,
        pins: &HashMap<PinKey, Pin>,
        index_cache: &mut HashMap<Key, Vec<SparseIndexEntry>>,
        excluded: &HashSet<(Key, String)>,
    ) -> Result<std::result::Result<CargoLock, Restart>> {
        let patches = self.patches.lock().unwrap().clone();
        let config = self.config.lock().unwrap().clone();
        let preferred = self.preferred.lock().unwrap().clone();
        let rust_limit = *self.rust_limit.lock().unwrap();
        let relaxed = self.rust_relaxed.lock().unwrap().clone();
        let relax_all = *self.rust_relax_all.lock().unwrap();
        // For each registry package requested by another registry package: that parent and
        // its version, to blame when the request has no solution.
        let mut parent_of: HashMap<Key, Vec<(Key, String)>> = HashMap::new();
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
                    // The version fits but lacks a feature this edge asks for: restart with
                    // the feature pinned, so the range picks a version that has it.
                    Some(Ok(i)) if !node_offers(&nodes[i], &r.features) => {
                        return Ok(Err(Restart {
                            key: (k, compat_range(&nodes[i].version)),
                            req: r.req.clone(),
                            features: r.features.clone(),
                        }));
                    }
                    Some(Ok(i)) => i,
                    Some(Err(conflicting)) => {
                        // A patched package has one version, which pins cannot move: a
                        // request it does not satisfy is a conflict of this branch, blamed on
                        // the versions that asked for it so the search can avoid them.
                        if patches.get(&r.source, &r.name).is_some() {
                            return Err(anyhow::Error::new(Unsatisfiable {
                                message: format!(
                                    "patch for `{}` has version {}, which does not satisfy the requirement `{}`",
                                    r.name, nodes[conflicting].version, r.req
                                ),
                                blame: parent_of.get(&k).cloned().unwrap_or_default(),
                            }));
                        }
                        // Same compatibility range, no shared version yet: restart with both
                        // requirements pinned, as a unifying resolver would.
                        return Ok(Err(Restart {
                            key: (k, compat_range(&nodes[conflicting].version)),
                            req: r.req.clone(),
                            features: r.features.clone(),
                        }));
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
                                // A conflict of this branch: a parent version that asked
                                // for it may be ruled out and another branch tried.
                                return Err(anyhow::Error::new(Unsatisfiable {
                                    message: format!(
                                        "patch for `{}` has version {}, which does not satisfy the requirement `{}`",
                                        r.name, primary.version, r.req
                                    ),
                                    blame: parent_of.get(&k).cloned().unwrap_or_default(),
                                }));
                            }
                            queue.extend(res.registry_deps);
                            extra.extend(res.packages);
                            Node::opaque(&primary)
                        } else {
                            // Without the versions ruled out by backtracking.
                            let entries: Vec<SparseIndexEntry> = index_cache[&k]
                                .iter()
                                .filter(|e| !excluded.contains(&(k.clone(), e.vers.clone())))
                                .cloned()
                                .collect();
                            let entries = &entries;
                            let locked = preferred.get(&k).cloned().unwrap_or_default();
                            let mut reqs = vec![r.req.clone()];
                            let mut wanted = r.features.clone();
                            // A pin belongs to the range of the version it was learnt on, which a
                            // wide requirement (`*`, `>=0.8, <2`) does not name: pick, apply the
                            // pin of the range the pick landed in, pick again, until a pick lands
                            // in a range whose pin is already applied (or has none).
                            let mut applied: HashSet<String> = HashSet::new();
                            // A decision is this package for this requirement: relaxing one
                            // version range leaves the package's other ranges under the limit.
                            let k_str = format!("{}|{}", key_string(&k), r.req);
                            let limit =
                                rust_limit.filter(|_| !relax_all && !relaxed.contains(&k_str));
                            let chosen = loop {
                                let Some(pick) =
                                    pick_version(entries, &reqs, &wanted, &locked, limit)
                                else {
                                    break None;
                                };
                                let range = compat_range(&pick.vers);
                                if !applied.insert(range.clone()) {
                                    break Some(pick);
                                }
                                let Some(pin) = pins.get(&(k.clone(), range)) else {
                                    break Some(pick);
                                };
                                reqs.extend(pin.reqs.iter().cloned());
                                wanted.extend(pin.features.iter().cloned());
                            };
                            let chosen = chosen.ok_or_else(|| {
                                let by = requested_by
                                    .get(&k)
                                    .map(|p| format!(" (required by {p})"))
                                    .unwrap_or_default();
                                let what = if index_cache[&k].is_empty() {
                                    "the crate is not in the index".to_string()
                                } else {
                                    format!(
                                        "no non-yanked version satisfies {}",
                                        reqs.join(" and ")
                                    )
                                };
                                anyhow::Error::new(Unsatisfiable {
                                    message: format!(
                                        "resolving {} {}{by} from {}: {what}",
                                        r.name, r.req, r.source
                                    ),
                                    blame: parent_of.get(&k).cloned().unwrap_or_default(),
                                })
                            })?;
                            // Where the Rust limit changes the pick: a candidate for relaxing
                            // it if the graph turns out to have no solution.
                            if rust_limit.is_some() {
                                let limited =
                                    pick_version(entries, &reqs, &wanted, &locked, rust_limit);
                                let free = pick_version(entries, &reqs, &wanted, &locked, None);
                                if limited.map(|e| &e.vers) != free.map(|e| &e.vers) {
                                    let mut affected = self.rust_affected.lock().unwrap();
                                    if !affected.contains(&k_str) {
                                        affected.push(k_str.clone());
                                    }
                                }
                            }
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
                    nodes[idx]
                        .dependencies
                        .insert((pkg_name.clone(), d.req.clone()));
                    requested_by
                        .entry((source.clone(), pkg_name.clone()))
                        .or_insert_with(|| parent.clone());
                    // A registry parent can be backtracked (another version tried); a path or
                    // workspace one cannot.
                    let parent_is_registry = nodes[idx]
                        .source
                        .as_deref()
                        .is_some_and(|s| s.starts_with("registry+") || s.starts_with("sparse+"));
                    if parent_is_registry {
                        parent_of
                            .entry((source.clone(), pkg_name.clone()))
                            .or_default()
                            .push((
                                (parent_source.clone(), nodes[idx].name.clone()),
                                nodes[idx].version.clone(),
                            ));
                    }
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
        let qualify = |n: &Node, (dep, edge_req): &(String, String)| -> String {
            let dep = dep.as_str();
            if dep.contains(' ') {
                return dep.to_string();
            }
            let Some(versions) = versions_by_name.get(dep).filter(|v| v.len() > 1) else {
                return dep.to_string();
            };
            // The edge's own requirement; an opaque node's edges carry none.
            let req = if edge_req.is_empty() {
                n.deps
                    .iter()
                    .find(|d| d.package.as_deref().unwrap_or(&d.name) == dep)
                    .and_then(|d| semver::VersionReq::parse(&d.req).ok())
            } else {
                semver::VersionReq::parse(edge_req).ok()
            };
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
                // Two edges to one version (a normal and a build dependency) are one entry.
                dependencies: n
                    .dependencies
                    .iter()
                    .map(|d| qualify(n, d))
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect(),
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
        // A crate the index does not have (404) has no versions: the request fails at pick
        // time, where the resolver can blame the version that asked for it and backtrack.
        let none_if_missing = |res: Result<Vec<SparseIndexEntry>>| match res {
            Err(e) if e.downcast_ref::<Unsatisfiable>().is_some() => Ok(Vec::new()),
            other => other,
        };
        if keys.len() == 1 {
            let k = &keys[0];
            let entries = none_if_missing(self.source_for(&k.0)?.versions(&k.1, offline))?;
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
            index_cache.insert(
                k.clone(),
                none_if_missing(slot.into_inner().unwrap().unwrap())?,
            );
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

#[cfg(test)]
#[path = "resolve_graph_pick_tests.rs"]
mod pick_tests;
