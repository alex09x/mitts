/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! `mitts tree -e features`: Cargo's feature graph. A dependency edge goes through the
//! features its declaration asks for (`log feature "default"`); a feature node leads to its
//! package and to the features, optional dependencies and dependency features it enables.
//! Only features that are active in the resolve appear. Edges inside the feature graph are
//! of kind `Feature`, which every edge filter follows.

use super::tree_graph::{EdgeKind, Graph};
use semver::{Version, VersionReq};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

const SEP: char = '\u{1f}';

/// The id of the node for feature `feature` of package `pkg`.
pub fn feature_id(pkg: &str, feature: &str) -> String {
    format!("feature{SEP}{pkg}{SEP}{feature}")
}

/// `(package id, feature)` for a feature node id.
pub fn split_feature_id(id: &str) -> Option<(&str, &str)> {
    let rest = id.strip_prefix(&format!("feature{SEP}"))?;
    rest.split_once(SEP)
}

/// The package a node belongs to: itself, or the owner of a feature node.
pub fn owner(id: &str) -> &str {
    split_feature_id(id).map_or(id, |(pkg, _)| pkg)
}

/// One resolved dependency edge with the declaration it came from.
struct DepEdge {
    child: String,
    kind: EdgeKind,
    /// The name the manifest knows it by (`rename`, else the package name).
    key: String,
    features: Vec<String>,
    default: bool,
}

struct Builder {
    graph: Graph,
    deps: BTreeMap<String, Vec<DepEdge>>,
    tables: BTreeMap<String, BTreeMap<String, Vec<String>>>,
    done: BTreeSet<String>,
}

fn kind_of(value: &Value) -> EdgeKind {
    match value.as_str() {
        Some("build") => EdgeKind::Build,
        Some("dev") => EdgeKind::Dev,
        _ => EdgeKind::Normal,
    }
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect()
}

/// The package graph of `doc` with feature nodes between packages.
pub fn graph(doc: &Value) -> Graph {
    let base = Graph::from_metadata(doc);
    let packages: BTreeMap<&str, &Value> = doc["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| Some((p["id"].as_str()?, p)))
        .collect();
    let mut tables = BTreeMap::new();
    for (id, p) in &packages {
        let table: BTreeMap<String, Vec<String>> = p["features"]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(k, v)| (k.clone(), strings(v)))
            .collect();
        tables.insert(id.to_string(), table);
    }
    let mut deps: BTreeMap<String, Vec<DepEdge>> = BTreeMap::new();
    for node in doc["resolve"]["nodes"].as_array().into_iter().flatten() {
        let parent = node["id"].as_str().unwrap_or_default();
        let declared = packages
            .get(parent)
            .and_then(|p| p["dependencies"].as_array())
            .cloned()
            .unwrap_or_default();
        let mut list = Vec::new();
        for dep in node["deps"].as_array().into_iter().flatten() {
            let child = dep["pkg"].as_str().unwrap_or_default();
            let extern_name = dep["name"].as_str().unwrap_or_default();
            let Some(child_pkg) = base.packages.get(child) else {
                continue;
            };
            let child_version = Version::parse(&child_pkg.version).ok();
            for dk in dep["dep_kinds"].as_array().into_iter().flatten() {
                let kind = kind_of(&dk["kind"]);
                let target = dk["target"].as_str();
                // The declaration this edge comes from (`name` is the package and `rename`
                // the manifest key, as Cargo's metadata has them): it names this package,
                // admits this version, has this kind and target, and a renamed one is the
                // extern name rustc sees.
                let decl = declared.iter().find(|d| {
                    kind_of(&d["kind"]) == kind
                        && d["target"].as_str() == target
                        && d["name"].as_str() == Some(child_pkg.name.as_str())
                        && match (d["req"].as_str(), &child_version) {
                            (Some(req), Some(v)) => {
                                VersionReq::parse(req).map_or(true, |r| r.matches(v))
                            }
                            _ => true,
                        }
                        && match d["rename"].as_str() {
                            Some(rename) => rename.replace('-', "_") == extern_name,
                            // Unrenamed: rustc sees the library's own name.
                            None => {
                                child_pkg
                                    .lib_name
                                    .as_deref()
                                    .unwrap_or(&child_pkg.name)
                                    .replace('-', "_")
                                    == extern_name
                            }
                        }
                });
                let Some(decl) = decl else {
                    continue;
                };
                list.push(DepEdge {
                    child: child.to_string(),
                    kind,
                    key: decl["rename"]
                        .as_str()
                        .or(decl["name"].as_str())
                        .unwrap_or_default()
                        .to_string(),
                    features: strings(&decl["features"]),
                    default: decl["uses_default_features"].as_bool().unwrap_or(true),
                });
            }
        }
        deps.insert(parent.to_string(), list);
    }
    let mut builder = Builder {
        graph: Graph {
            packages: base.packages.clone(),
            edges: BTreeMap::new(),
            root: base.root.clone(),
            members: base.members.clone(),
            labels: BTreeMap::new(),
        },
        deps,
        tables,
        done: BTreeSet::new(),
    };
    // The roots' own active features, so `-i` can show what enabled a dependency.
    for member in base.members.clone() {
        let active = builder
            .graph
            .packages
            .get(&member)
            .map(|p| p.features.clone())
            .unwrap_or_default();
        for feature in active {
            builder.feature(&member, &feature);
        }
    }
    let ids: Vec<String> = builder.deps.keys().cloned().collect();
    for id in ids {
        builder.package(&id);
    }
    let mut graph = builder.graph;
    graph.sort_edges();
    graph
}

impl Builder {
    fn active(&self, pkg: &str, feature: &str) -> bool {
        self.graph
            .packages
            .get(pkg)
            .is_some_and(|p| p.features.iter().any(|f| f == feature))
    }

    fn edge(&mut self, from: &str, to: String, kind: EdgeKind) {
        let list = self.graph.edges.entry(from.to_string()).or_default();
        if !list.contains(&(to.clone(), kind)) {
            list.push((to, kind));
        }
    }

    /// The resolved dependencies of `pkg` its manifest calls `name`, with the kind of each
    /// declaration: a feature that enables a build dependency reaches it by a build edge.
    fn dependency(&self, pkg: &str, name: &str) -> Vec<(String, EdgeKind)> {
        let mut out: Vec<(String, EdgeKind)> = self
            .deps
            .get(pkg)
            .into_iter()
            .flatten()
            .filter(|d| d.key == name)
            .map(|d| (d.child.clone(), d.kind))
            .collect();
        out.sort();
        out.dedup();
        out
    }

    fn package(&mut self, id: &str) {
        let edges: Vec<(String, EdgeKind, Vec<String>, bool)> = self
            .deps
            .get(id)
            .into_iter()
            .flatten()
            .map(|d| (d.child.clone(), d.kind, d.features.clone(), d.default))
            .collect();
        for (child, kind, features, default) in edges {
            let mut requested: BTreeSet<String> = features.into_iter().collect();
            if default {
                requested.insert("default".to_string());
            }
            requested.retain(|f| self.active(&child, f));
            if requested.is_empty() {
                self.edge(id, child.clone(), kind);
                continue;
            }
            for feature in requested {
                self.feature(&child, &feature);
                self.edge(id, feature_id(&child, &feature), kind);
            }
        }
    }

    fn feature(&mut self, pkg: &str, feature: &str) {
        let id = feature_id(pkg, feature);
        if !self.done.insert(id.clone()) {
            return;
        }
        let name = self
            .graph
            .packages
            .get(pkg)
            .map(|p| p.name.clone())
            .unwrap_or_default();
        self.graph
            .labels
            .insert(id.clone(), format!("{name} feature \"{feature}\""));
        self.edge(&id, pkg.to_string(), EdgeKind::Feature);
        let values = self.tables.get(pkg).and_then(|t| t.get(feature)).cloned();
        let Some(values) = values else {
            // An optional dependency's implicit feature.
            for (dep, kind) in self.dependency(pkg, feature) {
                self.edge(&id, dep, kind);
            }
            return;
        };
        for value in values {
            if let Some(dep_name) = value.strip_prefix("dep:") {
                for (dep, kind) in self.dependency(pkg, dep_name) {
                    self.edge(&id, dep, kind);
                }
            } else if let Some((dep_name, dep_feature)) = value.split_once('/') {
                for (dep, kind) in self.dependency(pkg, dep_name.trim_end_matches('?')) {
                    if self.active(&dep, dep_feature) {
                        self.feature(&dep, dep_feature);
                        self.edge(&id, feature_id(&dep, dep_feature), kind);
                    }
                }
            } else if self.active(pkg, &value) {
                self.feature(pkg, &value);
                self.edge(&id, feature_id(pkg, &value), EdgeKind::Feature);
            }
        }
    }
}

#[cfg(test)]
#[path = "tree_features_tests.rs"]
mod tests;
