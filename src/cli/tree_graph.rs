/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! The package graph `mitts tree` prints, read from the `cargo metadata` document of a
//! test-mode plan (so that dev-dependencies of workspace members are in it).

use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EdgeKind {
    Normal,
    Build,
    Dev,
    /// Inside the `-e features` graph (feature to its package, to features it enables):
    /// followed whatever kinds are selected, printed without a header.
    Feature,
}

impl EdgeKind {
    pub fn header(self) -> Option<&'static str> {
        match self {
            EdgeKind::Normal | EdgeKind::Feature => None,
            EdgeKind::Build => Some("[build-dependencies]"),
            EdgeKind::Dev => Some("[dev-dependencies]"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Package {
    pub name: String,
    pub version: String,
    pub source: Option<String>,
    pub dir: PathBuf,
    pub proc_macro: bool,
    pub workspace: bool,
    pub license: Option<String>,
    pub repository: Option<String>,
    pub features: Vec<String>,
    pub lib_name: Option<String>,
}

#[derive(Debug, Default)]
pub struct Graph {
    pub packages: BTreeMap<String, Package>,
    /// Outgoing edges: package id -> (dependency id, kind), sorted by the dependency's name.
    pub edges: BTreeMap<String, Vec<(String, EdgeKind)>>,
    pub root: Option<String>,
    pub members: Vec<String>,
    /// Nodes that are not packages (`-e features`: `log feature "std"`), with their text.
    pub labels: BTreeMap<String, String>,
}

impl Graph {
    pub fn from_metadata(doc: &Value) -> Graph {
        let mut graph = Graph::default();
        for p in doc["packages"].as_array().into_iter().flatten() {
            let id = p["id"].as_str().unwrap_or_default().to_string();
            let targets = p["targets"].as_array().cloned().unwrap_or_default();
            let lib = targets.iter().find(|t| {
                t["kind"].as_array().is_some_and(|k| {
                    k.iter()
                        .any(|k| k == "lib" || k == "proc-macro" || k == "rlib")
                })
            });
            let manifest = PathBuf::from(p["manifest_path"].as_str().unwrap_or_default());
            graph.packages.insert(
                id,
                Package {
                    name: p["name"].as_str().unwrap_or_default().to_string(),
                    version: p["version"].as_str().unwrap_or_default().to_string(),
                    source: p["source"].as_str().map(str::to_string),
                    dir: manifest.parent().map(PathBuf::from).unwrap_or_default(),
                    proc_macro: lib.is_some_and(|t| t["kind"][0] == "proc-macro"),
                    workspace: false,
                    license: p["license"].as_str().map(str::to_string),
                    repository: p["repository"].as_str().map(str::to_string),
                    features: Vec::new(),
                    lib_name: lib
                        .and_then(|t| t["name"].as_str())
                        .map(|n| n.replace('-', "_")),
                },
            );
        }
        for id in doc["workspace_members"].as_array().into_iter().flatten() {
            if let Some(id) = id.as_str() {
                graph.members.push(id.to_string());
                if let Some(p) = graph.packages.get_mut(id) {
                    p.workspace = true;
                }
            }
        }
        graph.root = doc["resolve"]["root"].as_str().map(str::to_string);
        for node in doc["resolve"]["nodes"].as_array().into_iter().flatten() {
            let id = node["id"].as_str().unwrap_or_default().to_string();
            if let Some(p) = graph.packages.get_mut(&id) {
                p.features = node["features"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|f| f.as_str().map(str::to_string))
                    .collect();
            }
            let mut out = Vec::new();
            for dep in node["deps"].as_array().into_iter().flatten() {
                let child = dep["pkg"].as_str().unwrap_or_default().to_string();
                for kind in dep["dep_kinds"].as_array().into_iter().flatten() {
                    let kind = match kind["kind"].as_str() {
                        Some("build") => EdgeKind::Build,
                        Some("dev") => EdgeKind::Dev,
                        _ => EdgeKind::Normal,
                    };
                    if !out.contains(&(child.clone(), kind)) {
                        out.push((child.clone(), kind));
                    }
                }
            }
            graph.edges.insert(id, out);
        }
        graph.sort_edges();
        graph
    }

    /// Edges in Cargo's order: by package name, then version; a package before its features,
    /// features by name.
    pub fn sort_edges(&mut self) {
        let packages = &self.packages;
        let key = |id: &str| -> (String, String, u8, String) {
            let (pkg, feature) = match super::tree_features::split_feature_id(id) {
                Some((pkg, feature)) => (pkg, Some(feature)),
                None => (id, None),
            };
            let (name, version) = packages
                .get(pkg)
                .map(|p| (p.name.clone(), p.version.clone()))
                .unwrap_or_default();
            match feature {
                Some(f) => (name, version, 1, f.to_string()),
                None => (name, version, 0, String::new()),
            }
        };
        for list in self.edges.values_mut() {
            list.sort_by(|a, b| key(&a.0).cmp(&key(&b.0)).then(a.1.cmp(&b.1)));
        }
    }

    /// The same graph with every edge reversed (`--invert`).
    pub fn inverted(&self) -> Graph {
        let mut edges: BTreeMap<String, Vec<(String, EdgeKind)>> = BTreeMap::new();
        for (from, list) in &self.edges {
            for (to, kind) in list {
                let entry = edges.entry(to.clone()).or_default();
                if !entry.contains(&(from.clone(), *kind)) {
                    entry.push((from.clone(), *kind));
                }
            }
        }
        let mut graph = Graph {
            packages: self.packages.clone(),
            edges,
            root: None,
            members: self.members.clone(),
            labels: self.labels.clone(),
        };
        graph.sort_edges();
        graph
    }

    /// Package ids matching a spec (`name`, `name@version`, `name:version`).
    pub fn matching(&self, spec: &str) -> Vec<String> {
        let (name, version) = super::project_info::parse_spec(spec);
        self.packages
            .iter()
            .filter(|(_, p)| {
                p.name == name
                    && version
                        .is_none_or(|v| p.version == v || p.version.starts_with(&format!("{v}.")))
            })
            .map(|(id, _)| id.clone())
            .collect()
    }

    pub fn has_edges(&self, id: &str, kinds: &[EdgeKind]) -> bool {
        self.edges
            .get(id)
            .is_some_and(|list| list.iter().any(|(_, k)| kinds.contains(k)))
    }
}
