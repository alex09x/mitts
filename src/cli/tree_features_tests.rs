/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::*;
use serde_json::json;

fn pkg(id: &str, version: &str, features: Value, deps: Value) -> Value {
    json!({"id": id, "name": id.split('@').next().unwrap(), "version": version,
           "source": "registry+x", "manifest_path": format!("/r/{id}/Cargo.toml"),
           "targets": [], "features": features, "dependencies": deps})
}

fn edges(g: &Graph, from: &str) -> Vec<(String, EdgeKind)> {
    g.edges.get(from).cloned().unwrap_or_default()
}

#[test]
fn dependency_edges_go_through_requested_features() {
    let doc = json!({
        "packages": [
            pkg("app", "0.1.0", json!({"default": ["dep:plain"]}), json!([
                {"name": "log", "req": "^0.4", "kind": null, "features": [], "uses_default_features": true},
                {"name": "plain", "req": "^1", "kind": null, "optional": true, "features": [], "uses_default_features": true}
            ])),
            pkg("log", "0.4.0", json!({"default": ["std"], "std": [], "kv": []}), json!([])),
            pkg("plain", "1.0.0", json!({}), json!([]))
        ],
        "workspace_members": ["app"],
        "resolve": {"root": "app", "nodes": [
            {"id": "app", "features": ["default"], "deps": [
                {"name": "log", "pkg": "log", "dep_kinds": [{"kind": null, "target": null}]},
                {"name": "plain", "pkg": "plain", "dep_kinds": [{"kind": null, "target": null}]}
            ]},
            {"id": "log", "features": ["default", "std"], "deps": []},
            {"id": "plain", "features": [], "deps": []}
        ]}
    });
    let g = graph(&doc);
    assert_eq!(
        edges(&g, "app"),
        [
            (feature_id("log", "default"), EdgeKind::Normal),
            ("plain".to_string(), EdgeKind::Normal)
        ]
    );
    assert_eq!(
        edges(&g, &feature_id("log", "default")),
        [
            ("log".to_string(), EdgeKind::Feature),
            (feature_id("log", "std"), EdgeKind::Feature)
        ]
    );
    assert!(
        !g.labels.contains_key(&feature_id("log", "kv")),
        "inactive features are left out"
    );
    // The root's own active feature is in the graph and leads to the optional dependency.
    assert_eq!(
        edges(&g, &feature_id("app", "default")),
        [
            ("app".to_string(), EdgeKind::Feature),
            ("plain".to_string(), EdgeKind::Feature)
        ]
    );
}

#[test]
fn features_belong_to_the_declaration_of_each_version() {
    // `old` is a rename of rand 0.7 asking for "small"; `rand` 0.8 asks for "std".
    let doc = json!({
        "packages": [
            pkg("app", "0.1.0", json!({}), json!([
                {"name": "rand", "rename": "old", "req": "^0.7", "kind": null, "features": ["small"], "uses_default_features": false},
                {"name": "rand", "req": "^0.8", "kind": null, "features": ["std"], "uses_default_features": false}
            ])),
            pkg("rand@0.7", "0.7.3", json!({"small": [], "std": []}), json!([])),
            pkg("rand@0.8", "0.8.5", json!({"small": [], "std": []}), json!([]))
        ],
        "workspace_members": ["app"],
        "resolve": {"root": "app", "nodes": [
            {"id": "app", "features": [], "deps": [
                {"name": "old", "pkg": "rand@0.7", "dep_kinds": [{"kind": null, "target": null}]},
                {"name": "rand", "pkg": "rand@0.8", "dep_kinds": [{"kind": null, "target": null}]}
            ]},
            {"id": "rand@0.7", "features": ["small", "std"], "deps": []},
            {"id": "rand@0.8", "features": ["small", "std"], "deps": []}
        ]}
    });
    let g = graph(&doc);
    let to: Vec<String> = edges(&g, "app").into_iter().map(|(t, _)| t).collect();
    assert_eq!(
        to,
        [
            feature_id("rand@0.7", "small"),
            feature_id("rand@0.8", "std")
        ]
    );
}
