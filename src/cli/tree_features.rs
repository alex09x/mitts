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
//! features the dependent asks for (`log feature "default"`); a feature node leads to its
//! package and to the features, optional dependencies and dependency features it enables.
//! Only features that are active in the resolve appear.

use super::tree_graph::{EdgeKind, Graph};
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

struct Declared {
    name: String,
    rename: Option<String>,
    kind: EdgeKind,
    features: Vec<String>,
    default: bool,
}

struct Builder<'a> {
    graph: Graph,
    declared: BTreeMap<String, Vec<Declared>>,
    tables: BTreeMap<String, BTreeMap<String, Vec<String>>>,
    package_edges: BTreeMap<String, Vec<(String, EdgeKind)>>,
    done: BTreeSet<String>,
    _doc: &'a Value,
}

/// The package graph of `doc` with feature nodes between packages.
pub fn graph(doc: &Value) -> Graph {
    let base = Graph::from_metadata(doc);
    let mut declared: BTreeMap<String, Vec<Declared>> = BTreeMap::new();
    let mut tables = BTreeMap::new();
    for p in doc["packages"].as_array().into_iter().flatten() {
        let id = p["id"].as_str().unwrap_or_default().to_string();
        let mut list = Vec::new();
        for d in p["dependencies"].as_array().into_iter().flatten() {
            list.push(Declared {
                name: d["name"].as_str().unwrap_or_default().to_string(),
                rename: d["rename"].as_str().map(str::to_string),
                kind: match d["kind"].as_str() {
                    Some("build") => EdgeKind::Build,
                    Some("dev") => EdgeKind::Dev,
                    _ => EdgeKind::Normal,
                },
                features: d["features"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|f| f.as_str().map(str::to_string))
                    .collect(),
                default: d["uses_default_features"].as_bool().unwrap_or(true),
            });
        }
        declared.insert(id.clone(), list);
        let table: BTreeMap<String, Vec<String>> = p["features"]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(k, v)| {
                let values = v
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect();
                (k.clone(), values)
            })
            .collect();
        tables.insert(id, table);
    }
    let package_edges = base.edges.clone();
    let mut builder = Builder {
        graph: Graph {
            packages: base.packages.clone(),
            edges: BTreeMap::new(),
            root: base.root.clone(),
            members: base.members.clone(),
            labels: BTreeMap::new(),
        },
        declared,
        tables,
        package_edges,
        done: BTreeSet::new(),
        _doc: doc,
    };
    let ids: Vec<String> = builder.package_edges.keys().cloned().collect();
    for id in ids {
        builder.package(&id);
    }
    let mut graph = builder.graph;
    graph.sort_edges();
    graph
}

impl Builder<'_> {
    fn active(&self, pkg: &str, feature: &str) -> bool {
        self.graph
            .packages
            .get(pkg)
            .is_some_and(|p| p.features.iter().any(|f| f == feature))
    }

    fn name_of(&self, pkg: &str) -> &str {
        self.graph
            .packages
            .get(pkg)
            .map(|p| p.name.as_str())
            .unwrap_or_default()
    }

    fn edge(&mut self, from: &str, to: String, kind: EdgeKind) {
        let list = self.graph.edges.entry(from.to_string()).or_default();
        if !list.contains(&(to.clone(), kind)) {
            list.push((to, kind));
        }
    }

    /// The resolved dependency of `pkg` that `name` (package name or rename) refers to.
    fn dependency(&self, pkg: &str, name: &str) -> Option<String> {
        let declared = self.declared.get(pkg)?;
        let package_name = declared
            .iter()
            .find(|d| d.rename.as_deref() == Some(name) || (d.rename.is_none() && d.name == name))
            .map(|d| d.name.clone())?;
        self.package_edges
            .get(pkg)?
            .iter()
            .map(|(child, _)| child)
            .find(|child| self.name_of(child) == package_name)
            .cloned()
    }

    fn package(&mut self, id: &str) {
        let children = self.package_edges.get(id).cloned().unwrap_or_default();
        for (child, kind) in children {
            let child_name = self.name_of(&child).to_string();
            let mut requested: BTreeSet<String> = BTreeSet::new();
            for d in self.declared.get(id).into_iter().flatten() {
                if d.name == child_name && d.kind == kind {
                    requested.extend(d.features.iter().cloned());
                    if d.default {
                        requested.insert("default".to_string());
                    }
                }
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
        let name = self.name_of(pkg).to_string();
        self.graph
            .labels
            .insert(id.clone(), format!("{name} feature \"{feature}\""));
        self.edge(&id, pkg.to_string(), EdgeKind::Normal);
        let values = self.tables.get(pkg).and_then(|t| t.get(feature)).cloned();
        let Some(values) = values else {
            // An optional dependency's implicit feature.
            if let Some(dep) = self.dependency(pkg, feature) {
                self.edge(&id, dep, EdgeKind::Normal);
            }
            return;
        };
        for value in values {
            if let Some(dep_name) = value.strip_prefix("dep:") {
                if let Some(dep) = self.dependency(pkg, dep_name) {
                    self.edge(&id, dep, EdgeKind::Normal);
                }
            } else if let Some((dep_name, dep_feature)) = value.split_once('/') {
                let dep_name = dep_name.trim_end_matches('?');
                if let Some(dep) = self.dependency(pkg, dep_name) {
                    if self.active(&dep, dep_feature) {
                        self.feature(&dep, dep_feature);
                        self.edge(&id, feature_id(&dep, dep_feature), EdgeKind::Normal);
                    }
                }
            } else if self.active(pkg, &value) {
                self.feature(pkg, &value);
                self.edge(&id, feature_id(pkg, &value), EdgeKind::Normal);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn dependency_edges_go_through_requested_features() {
        let doc = json!({
            "packages": [
                {"id": "app", "name": "app", "version": "0.1.0", "manifest_path": "/w/app/Cargo.toml",
                 "targets": [], "features": {},
                 "dependencies": [
                    {"name": "log", "kind": null, "features": [], "uses_default_features": true},
                    {"name": "plain", "kind": null, "features": [], "uses_default_features": true}
                 ]},
                {"id": "log", "name": "log", "version": "0.4.0", "source": "registry+x",
                 "manifest_path": "/r/log/Cargo.toml", "targets": [],
                 "features": {"default": ["std"], "std": [], "kv": []}, "dependencies": []},
                {"id": "plain", "name": "plain", "version": "1.0.0", "source": "registry+x",
                 "manifest_path": "/r/plain/Cargo.toml", "targets": [], "features": {}, "dependencies": []}
            ],
            "workspace_members": ["app"],
            "resolve": {"root": "app", "nodes": [
                {"id": "app", "features": [], "deps": [
                    {"pkg": "log", "dep_kinds": [{"kind": null}]},
                    {"pkg": "plain", "dep_kinds": [{"kind": null}]}
                ]},
                {"id": "log", "features": ["default", "std"], "deps": []},
                {"id": "plain", "features": [], "deps": []}
            ]}
        });
        let g = graph(&doc);
        let app: Vec<&str> = g.edges["app"].iter().map(|(c, _)| c.as_str()).collect();
        assert_eq!(app, [feature_id("log", "default").as_str(), "plain"]);
        let default: Vec<&str> = g.edges[&feature_id("log", "default")]
            .iter()
            .map(|(c, _)| c.as_str())
            .collect();
        assert_eq!(default, ["log", feature_id("log", "std").as_str()]);
        assert_eq!(g.labels[&feature_id("log", "std")], "log feature \"std\"");
        assert!(
            !g.labels.contains_key(&feature_id("log", "kv")),
            "inactive features are left out"
        );
    }
}
