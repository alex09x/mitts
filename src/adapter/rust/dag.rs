/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::types::{EffectivePlanExplanation, PlannedCompilationUnit};
use anyhow::{bail, Result};
use std::collections::{HashMap, HashSet};

/// Represents a single compilation node within the Mitts dependency DAG.
#[derive(Debug, Clone)]
pub struct DagNode {
    pub unit: PlannedCompilationUnit,
    pub dependencies: Vec<String>,
    pub dependents: Vec<String>,
}

/// Represents the directed acyclic graph (DAG) of compilation units.
#[derive(Debug, Clone)]
pub struct CompilationDag {
    nodes: HashMap<String, DagNode>,
    roots: Vec<String>,
    leaves: Vec<String>,
}

impl CompilationDag {
    /// Builds a `CompilationDag` from an `EffectivePlanExplanation`.
    pub fn from_explanation(explanation: &EffectivePlanExplanation) -> Result<Self> {
        let known_ids: HashSet<String> = explanation.units.iter().map(|u| u.unit_id()).collect();

        let mut nodes: HashMap<String, DagNode> = HashMap::new();

        for unit in &explanation.units {
            let u_id = unit.unit_id();
            // Filter dependencies to only those that exist in the planned compilation units
            let valid_deps: Vec<String> = unit
                .dependencies
                .iter()
                .filter(|d| known_ids.contains(*d))
                .cloned()
                .collect();

            nodes.insert(
                u_id,
                DagNode {
                    unit: unit.clone(),
                    dependencies: valid_deps,
                    dependents: Vec::new(),
                },
            );
        }

        // Linking targets (bin, test, cdylib, example, bench, dylib, proc-macro) link against machine code
        // (.rlib) of all transitive dependencies. Expand their DAG dependencies transitively so they
        // only start execution after all transitive libraries have finished producing .rlib.
        let linking_ids: Vec<String> = nodes
            .iter()
            .filter(|(_, node)| {
                matches!(
                    node.unit.kind.as_str(),
                    "bin"
                        | "test"
                        | "custom-test"
                        | "example"
                        | "bench"
                        | "cdylib"
                        | "dylib"
                        | "proc-macro"
                ) || super::pipelining::package_has_build_script(&node.unit.manifest_path)
            })
            .map(|(id, _)| id.clone())
            .collect();

        for id in linking_ids {
            let mut transitive = HashSet::new();
            let mut stack = nodes[&id].dependencies.clone();
            while let Some(curr) = stack.pop() {
                if transitive.insert(curr.clone()) {
                    if let Some(dep_node) = nodes.get(&curr) {
                        stack.extend(dep_node.dependencies.clone());
                    }
                }
            }
            if let Some(node) = nodes.get_mut(&id) {
                let mut all_deps: Vec<String> = transitive.into_iter().collect();
                all_deps.sort();
                node.dependencies = all_deps;
            }
        }

        // Populate reverse dependents edges
        let node_keys: Vec<String> = nodes.keys().cloned().collect();
        for id in &node_keys {
            let deps = nodes[id].dependencies.clone();
            for dep_id in deps {
                if let Some(dep_node) = nodes.get_mut(&dep_id) {
                    dep_node.dependents.push(id.clone());
                }
            }
        }

        // Roots: units that no other unit depends on (typically top-level binaries/libraries)
        let roots: Vec<String> = nodes
            .iter()
            .filter(|(_, node)| node.dependents.is_empty())
            .map(|(id, _)| id.clone())
            .collect();

        // Leaves: units that have 0 dependencies (can be compiled immediately in parallel)
        let leaves: Vec<String> = nodes
            .iter()
            .filter(|(_, node)| node.dependencies.is_empty())
            .map(|(id, _)| id.clone())
            .collect();

        let dag = Self {
            nodes,
            roots,
            leaves,
        };

        // Validate acyclicity via topological wave computation
        let _ = dag.topological_waves()?;

        Ok(dag)
    }

    /// Returns a reference to a DAG node by its unit ID.
    pub fn get_node(&self, unit_id: &str) -> Option<&DagNode> {
        self.nodes.get(unit_id)
    }

    /// Returns all node IDs in the DAG.
    pub fn node_ids(&self) -> Vec<String> {
        self.nodes.keys().cloned().collect()
    }

    /// Returns the root units (units with no dependents).
    pub fn root_ids(&self) -> &[String] {
        &self.roots
    }

    /// Returns the leaf units (units with no dependencies, ready to compile).
    pub fn leaf_ids(&self) -> &[String] {
        &self.leaves
    }

    /// Total number of compilation units in this DAG.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Checks whether the DAG is empty.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Partitions the DAG into topological execution waves.
    /// Units in each wave are guaranteed to only depend on units from strictly earlier waves,
    /// enabling parallel multi-threaded compilation within each wave.
    pub fn topological_waves(&self) -> Result<Vec<Vec<String>>> {
        let mut in_degrees: HashMap<String, usize> = HashMap::new();
        for (id, node) in &self.nodes {
            in_degrees.insert(id.clone(), node.dependencies.len());
        }

        let mut waves: Vec<Vec<String>> = Vec::new();
        let mut completed: HashSet<String> = HashSet::new();

        while completed.len() < self.nodes.len() {
            let current_wave: Vec<String> = self
                .nodes
                .keys()
                .filter(|id| !completed.contains(*id) && in_degrees[*id] == 0)
                .cloned()
                .collect();

            if current_wave.is_empty() {
                bail!("Cyclic dependency detected in compilation DAG");
            }

            for id in &current_wave {
                completed.insert(id.clone());
                if let Some(node) = self.nodes.get(id) {
                    for dep_on_me in &node.dependents {
                        if let Some(deg) = in_degrees.get_mut(dep_on_me) {
                            if *deg > 0 {
                                *deg -= 1;
                            }
                        }
                    }
                }
            }

            waves.push(current_wave);
        }

        Ok(waves)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::action::ToolchainInfo;
    use std::path::PathBuf;

    #[test]
    fn test_topological_waves_linear() {
        let u1 = PlannedCompilationUnit {
            package_id: "pkg1".to_string(),
            package_name: "pkg1".to_string(),
            version: "1.0.0".to_string(),
            target_name: "pkg1".to_string(),
            kind: "lib".to_string(),
            src_path: PathBuf::from("src/lib.rs"),
            manifest_path: PathBuf::from("Cargo.toml"),
            cache_eligibility: "eligible".to_string(),
            is_workspace: true,
            is_primary: true,
            env_vars: Vec::new(),
            dependencies: Vec::new(),
            edition: Some("2021".to_string()),
            features: Vec::new(),
            for_host: false,
            workspace_root: None,
        };
        let u2 = PlannedCompilationUnit {
            package_id: "pkg2".to_string(),
            package_name: "pkg2".to_string(),
            version: "1.0.0".to_string(),
            target_name: "pkg2".to_string(),
            kind: "bin".to_string(),
            src_path: PathBuf::from("src/main.rs"),
            manifest_path: PathBuf::from("Cargo.toml"),
            cache_eligibility: "eligible".to_string(),
            is_workspace: true,
            is_primary: true,
            env_vars: Vec::new(),
            dependencies: vec![u1.unit_id()],
            edition: Some("2021".to_string()),
            features: Vec::new(),
            for_host: false,
            workspace_root: None,
        };

        let explanation = EffectivePlanExplanation {
            manifest_snapshot: Default::default(),
            manifest_path: PathBuf::from("Cargo.toml"),
            target_directory: PathBuf::from("target"),
            profile: "debug".to_string(),
            toolchain: ToolchainInfo {
                name: "rustc".to_string(),
                version: "1.80.0".to_string(),
                target_triple: Some("x86_64".to_string()),
            },
            units: vec![u2.clone(), u1.clone()],
        };

        let dag = CompilationDag::from_explanation(&explanation).unwrap();
        assert_eq!(dag.len(), 2);
        assert_eq!(dag.leaf_ids(), &[u1.unit_id()]);
        assert_eq!(dag.root_ids(), &[u2.unit_id()]);

        let waves = dag.topological_waves().unwrap();
        assert_eq!(waves.len(), 2);
        assert_eq!(waves[0], vec![u1.unit_id()]);
        assert_eq!(waves[1], vec![u2.unit_id()]);
    }
}
