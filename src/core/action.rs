/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

/// Distinct type of compilation/build action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActionKind {
    CompileRlib,
    CompileBin,
    BuildScript,
    Custom(String),
}

/// A language-neutral build action representing a discrete compilation unit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Action {
    pub id: String,
    pub kind: ActionKind,
    pub description: String,
    pub toolchain: ToolchainInfo,
    pub inputs: Vec<PathBuf>,
    pub outputs: Vec<PathBuf>,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    pub dependencies: Vec<String>,
}

/// Toolchain identification independent of specific compiler details.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolchainInfo {
    pub name: String,
    pub version: String,
    pub target_triple: Option<String>,
}

/// Result of executing an action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionResult {
    pub action_id: String,
    pub exit_code: i32,
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
    pub cached: bool,
}

/// Full execution plan composed of ordered actions.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BuildPlan {
    pub actions: Vec<Action>,
    pub target_directory: PathBuf,
    /// Linkable artifact per action id, exactly as dependents consume it
    /// (e.g. the `.rlib`/proc-macro library passed via `--extern`).
    /// Empty when the adapter does not track it (e.g. fast-lock hits).
    #[serde(default)]
    pub artifacts: BTreeMap<String, PathBuf>,
    /// Build cfgs / directives per action id (e.g. rustc-cfg from build scripts).
    #[serde(default)]
    pub unit_cfgs: BTreeMap<String, Vec<String>>,
}
