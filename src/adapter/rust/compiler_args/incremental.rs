/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Profile `incremental` for workspace units, as Cargo applies it.
//!
//! The session state lives in `<target>/<debug|release>/incremental`, a per-checkout scratch
//! directory: it is never an input of an action key and never enters the shared cache. A
//! cache hit does not touch it; a miss (an edit) hands it to rustc, which then recompiles
//! only what the edit changed instead of the whole crate.

use crate::adapter::rust::compiler::RustcInvocation;
use crate::adapter::rust::profile::UnitProfileSettings;
use crate::adapter::rust::types::PlannedCompilationUnit;
use crate::core::hash::Sha256;
use std::path::PathBuf;

/// Whether `unit` compiles incrementally under `profile`: workspace units only, like Cargo,
/// so registry, git and vendored dependencies stay byte-for-byte shareable.
pub(crate) fn applies(unit: &PlannedCompilationUnit, profile: &UnitProfileSettings) -> bool {
    unit.is_workspace && profile.incremental
}

/// `-C incremental` directory for this invocation, or `None` when it does not apply.
pub(crate) fn session_root(
    inv: &RustcInvocation,
    profile: &UnitProfileSettings,
) -> Option<PathBuf> {
    if !applies(inv.unit, profile) {
        return None;
    }
    let profile_dir = if inv.release { "release" } else { "debug" };
    inv.target_dir
        .map(|t| t.join(profile_dir).join("incremental"))
}

/// A `-C metadata` value that stays the same across source edits of one unit, so rustc
/// finds its previous session. It covers what distinguishes two builds of the unit that
/// must not share a session or a crate identity: the unit itself (package, kind, target,
/// host), its features, the build mode and profile, and the extra compiler arguments.
pub(crate) fn stable_metadata_key(inv: &RustcInvocation, profile: &UnitProfileSettings) -> String {
    let mut features: Vec<&str> = inv.features.iter().map(String::as_str).collect();
    features.sort_unstable();
    let identity = format!(
        "mitts-incremental-v1\n{}\n{:?}\n{:?}\nrelease={}\n{:?}\n{:?}",
        inv.unit.unit_id(),
        features,
        inv.mode,
        inv.release,
        profile,
        inv.extra_compiler_args,
    );
    Sha256::digest(identity.as_bytes())[..16].to_string()
}
