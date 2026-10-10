/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::rust::dag::CompilationDag;
use mitts::adapter::rust::manifest::CargoToml;
use mitts::adapter::rust::types::{EffectivePlanExplanation, PlannedCompilationUnit};
use std::path::PathBuf;

fn make_unit(pkg: &str, target: &str, kind: &str, deps: Vec<String>) -> PlannedCompilationUnit {
    PlannedCompilationUnit {
        package_id: format!("{} 0.1.0", pkg),
        package_name: pkg.to_string(),
        version: "0.1.0".to_string(),
        target_name: target.to_string(),
        kind: kind.to_string(),
        src_path: PathBuf::from("src/lib.rs"),
        manifest_path: PathBuf::from("Cargo.toml"),
        cache_eligibility: "eligible".to_string(),
        is_workspace: true,
        is_primary: false,
        env_vars: vec![],
        dependencies: deps,
        edition: Some("2021".to_string()),
        features: vec![],
        for_host: false,
        workspace_root: None,
    }
}

#[test]
fn test_m8_dag_empty_and_cycle_detection() {
    let dummy_tc = mitts::core::action::ToolchainInfo {
        name: "rustc".to_string(),
        version: "rustc 1.80.0".to_string(),
        target_triple: None,
    };

    // 1. Empty plan
    let empty_plan = EffectivePlanExplanation {
        manifest_path: PathBuf::from("Cargo.toml"),
        target_directory: PathBuf::from("target"),
        profile: "dev".to_string(),
        toolchain: dummy_tc.clone(),
        units: vec![],
    };
    let dag = CompilationDag::from_explanation(&empty_plan).expect("empty dag");
    assert!(dag.is_empty());
    assert_eq!(dag.len(), 0);
    assert_eq!(dag.leaf_ids(), &[] as &[String]);
    assert_eq!(dag.root_ids(), &[] as &[String]);

    // 2. Cycle detection: A depends on B, B depends on A
    let u_a = make_unit("pkg-a", "a", "lib", vec!["pkg-b 0.1.0::lib:b".to_string()]);
    let u_b = make_unit("pkg-b", "b", "lib", vec!["pkg-a 0.1.0::lib:a".to_string()]);
    let cycle_plan = EffectivePlanExplanation {
        manifest_path: PathBuf::from("Cargo.toml"),
        target_directory: PathBuf::from("target"),
        profile: "dev".to_string(),
        toolchain: dummy_tc.clone(),
        units: vec![u_a, u_b],
    };
    let res = CompilationDag::from_explanation(&cycle_plan);
    assert!(res.is_err(), "Expected cycle detection error");
    let err_str = res.unwrap_err().to_string();
    assert!(err_str.contains("Cyclic dependency"));
}

#[test]
fn test_m8_dag_topological_waves_and_roots() {
    let dummy_tc = mitts::core::action::ToolchainInfo {
        name: "rustc".to_string(),
        version: "rustc 1.80.0".to_string(),
        target_triple: None,
    };

    // A (leaf) <- B (mid) <- C (root)
    let u_a = make_unit("pkg-a", "a", "lib", vec![]);
    let u_b = make_unit("pkg-b", "b", "lib", vec![u_a.unit_id()]);
    let u_c = make_unit("pkg-c", "c", "bin", vec![u_b.unit_id()]);

    let plan = EffectivePlanExplanation {
        manifest_path: PathBuf::from("Cargo.toml"),
        target_directory: PathBuf::from("target"),
        profile: "dev".to_string(),
        toolchain: dummy_tc,
        units: vec![u_c.clone(), u_a.clone(), u_b.clone()],
    };
    let dag = CompilationDag::from_explanation(&plan).expect("dag");
    assert_eq!(dag.len(), 3);
    assert_eq!(dag.leaf_ids(), &[u_a.unit_id()]);
    assert_eq!(dag.root_ids(), &[u_c.unit_id()]);

    let node_b = dag.get_node(&u_b.unit_id()).expect("node b");
    assert_eq!(node_b.dependencies, vec![u_a.unit_id()]);
    assert_eq!(node_b.dependents, vec![u_c.unit_id()]);

    let waves = dag.topological_waves().expect("waves");
    assert_eq!(waves.len(), 3);
    assert_eq!(waves[0], vec![u_a.unit_id()]);
    assert_eq!(waves[1], vec![u_b.unit_id()]);
    assert_eq!(waves[2], vec![u_c.unit_id()]);
}

#[test]
fn test_m8_manifest_dependency_aliases_and_renames() {
    let toml_str = r#"
[package]
name = "alias-test"
version = "0.1.0"
edition = "2021"

[dependencies]
serde_custom = { package = "serde", version = "1.0" }
tokio_renamed = { package = "tokio", version = "1.20" }
regular = "0.2"
"#;
    let cargo_toml: CargoToml = toml::from_str(toml_str).expect("parse toml");
    let aliases = cargo_toml.dependency_aliases();

    let serde_aliases = aliases.real_to_aliases.get("serde").expect("serde aliases");
    assert_eq!(serde_aliases.len(), 1);
    assert_eq!(serde_aliases[0].0, "serde_custom");
    assert_eq!(serde_aliases[0].1, Some("1.0".to_string()));

    let tokio_aliases = aliases.real_to_aliases.get("tokio").expect("tokio aliases");
    assert_eq!(tokio_aliases[0].0, "tokio_renamed");

    assert_eq!(aliases.alias_to_real.get("serde_custom").unwrap(), "serde");
    assert_eq!(aliases.alias_to_real.get("tokio_renamed").unwrap(), "tokio");
}
