/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::compiler::RustcInvocation;
use super::compiler_resolve::find_workspace_root;
use std::path::Path;
use std::process::Command;

// Version the normalized semantics, never the machine-specific alias spellings.
pub(crate) const KEY_FLAG: &str = "REMAP_ALIASES=canonical-v1";

/// rustc and the Darwin linker can canonicalize paths independently of the
/// manifest spelling (notably /var -> /private/var). Cover both spellings so
/// debug metadata and macro spans cannot retain the checkout's absolute root.
fn append_aliases(cmd: &mut Command, path: &Path, replacement: &str) {
    append_prefix(cmd, path, replacement);
    if let Ok(canonical) = path.canonicalize() {
        if canonical != path {
            append_prefix(cmd, &canonical, replacement);
        }
    }
}

fn append_prefix(cmd: &mut Command, path: &Path, replacement: &str) {
    let path = path.to_string_lossy();
    let prefix = path.trim_end_matches('/');
    let with_separator = if replacement.is_empty() {
        String::new()
    } else {
        format!("{replacement}/")
    };
    cmd.arg(format!("--remap-path-prefix={prefix}/={with_separator}"));
    if !prefix.is_empty() {
        cmd.arg(format!("--remap-path-prefix={prefix}={replacement}"));
    }
}

pub(crate) fn append_path_remaps(
    inv: &RustcInvocation<'_>,
    clean_crate: &str,
    cmd: &mut Command,
    pkg_dir: &Path,
) {
    if !super::inputs::is_immutable_package_dir(pkg_dir, inv.unit.is_workspace) {
        let ws_root = find_workspace_root(&inv.unit.manifest_path);
        if pkg_dir.starts_with(&ws_root) {
            append_aliases(cmd, &ws_root, "");
        }
    }

    // More specific output mappings follow the workspace mapping: rustc gives
    // the last matching rule precedence when the cache is inside the workspace.
    let stable_out = format!("/mitts/out/{clean_crate}");
    for out in std::iter::once(inv.out_dir).chain(
        inv.directives
            .and_then(|directives| directives.out_dir.as_deref()),
    ) {
        append_aliases(cmd, out, &stable_out);
    }
}
