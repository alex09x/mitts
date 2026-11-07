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
use std::path::{Path, PathBuf};
use std::process::Command;

// Version the normalized semantics, never the machine-specific alias spellings.
// The Cargo-compatible layout changes Darwin's relative N_OSO dependency paths;
// v4 switches composite unit keys to full-key metadata hashes for hermetic dependency resolution.
pub(crate) const KEY_FLAG: &str = "REMAP_ALIASES=canonical-v4-hermetic-deps";

/// Versions how coverage builds name source files; part of the unit key of instrumented units.
pub(crate) const COVERAGE_KEY_FLAG: &str = "COVERAGE_PATHS=build-root-relative-v1";

/// Whether the compiler arguments request source-based coverage instrumentation.
pub(crate) fn is_coverage_build(args: &[String]) -> bool {
    args.iter().any(|arg| arg == "-Cinstrument-coverage")
        || args
            .windows(2)
            .any(|pair| pair[0] == "-C" && pair[1] == "instrument-coverage")
}

/// Coverage reports name a file by the path rustc recorded, and the remap decides that path.
/// Dropping every package's own root gave every crate the same `src/lib.rs`, so a report could
/// neither tell the crates apart nor exclude one of them. Under the build's workspace root
/// every package keeps its path relative to that root, whether it is a member, a path
/// dependency with its own lockfile, or a vendored copy; a package outside the root gets a
/// distinct `external/<name>-<version>` prefix. Registry and git sources are not mutable
/// workspace packages and keep absolute paths, which the report layer excludes by default.
///
/// The recorded names must be unique on their own: LLVM merges coverage by file name, and joins
/// the recorded compilation directory (rustc's working directory) with a relative name. The
/// working directory of an instrumented unit is therefore the build root, see
/// [`coverage_working_dir`], which the first rule maps to nothing.
fn append_coverage_source_remap(inv: &RustcInvocation<'_>, cmd: &mut Command, pkg_dir: &Path) {
    let build_root = inv
        .unit
        .workspace_root
        .clone()
        .unwrap_or_else(|| find_workspace_root(&inv.unit.manifest_path));
    append_aliases(cmd, &build_root, "");

    // Compare real locations: a dependency path such as `app/../outsider` starts with `app`
    // component by component, yet lies outside it. The rule below also covers a package under
    // the root that was reached through a path with `..` in it.
    let canonical_root = build_root.canonicalize().unwrap_or(build_root);
    let canonical_pkg = pkg_dir
        .canonicalize()
        .unwrap_or_else(|_| pkg_dir.to_path_buf());
    let label = match canonical_pkg.strip_prefix(&canonical_root) {
        Ok(relative) => relative.to_string_lossy().to_string(),
        Err(_) => format!("external/{}-{}", inv.unit.package_name, inv.unit.version),
    };
    if !label.is_empty() {
        append_aliases(cmd, pkg_dir, &label);
    }
}

/// The directory rustc runs in for an instrumented unit: the build root, so the compilation
/// directory recorded in the coverage mapping is empty and file names alone identify files.
pub(crate) fn coverage_working_dir(inv: &RustcInvocation<'_>) -> Option<PathBuf> {
    if !is_coverage_build(inv.extra_compiler_args) || inv.unit.for_host {
        return None;
    }
    inv.unit.workspace_root.clone()
}

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

/// The Darwin linker records every object file's absolute path in the output's N_OSO
/// stabs (that is how lldb finds unpacked debug info); `--remap-path-prefix` does not
/// reach them, so every binary linked in a checkout embedded the checkout's path and
/// could never be shared with another one. `-oso_prefix` makes the linker store those
/// paths relative to the workspace root: the same string in every checkout, and still
/// resolvable by a debugger started there.
fn append_darwin_oso_prefix(inv: &RustcInvocation<'_>, cmd: &mut Command) {
    if !cfg!(target_os = "macos") {
        return;
    }
    // The paths the linker records are the ones rustc received: rlibs under
    // `<checkout>/target/…/deps` (the `-L dependency` directory) and archives build scripts
    // leave under `<checkout>/target/…/build`. The checkout spelling is whatever precedes
    // that `target` component, whichever package (workspace member, registry proc macro)
    // is being linked.
    let checkout_of = |p: &Path| -> Option<PathBuf> {
        p.ancestors()
            .find(|a| a.file_name().is_some_and(|n| n == "target"))
            .and_then(|t| t.parent())
            .map(Path::to_path_buf)
    };
    let root = inv
        .deps_dir
        .and_then(checkout_of)
        .or_else(|| inv.target_dir.and_then(checkout_of));
    let Some(root) = root else {
        return;
    };
    // ld records physical paths (notably /private/var instead of /var).
    let root = root.canonicalize().unwrap_or(root);
    let prefix = format!("{}/", root.to_string_lossy().trim_end_matches('/'));
    cmd.args(["-C", &format!("link-arg=-Wl,-oso_prefix,{prefix}")]);
}

pub(crate) fn append_path_remaps(
    inv: &RustcInvocation<'_>,
    clean_crate: &str,
    cmd: &mut Command,
    pkg_dir: &Path,
) {
    if !super::inputs::is_immutable_package_dir(pkg_dir, inv.unit.is_workspace) {
        if is_coverage_build(inv.extra_compiler_args) {
            append_coverage_source_remap(inv, cmd, pkg_dir);
        } else {
            let ws_root = find_workspace_root(&inv.unit.manifest_path);
            if pkg_dir.starts_with(&ws_root) {
                append_aliases(cmd, &ws_root, "");
            }
        }
    }
    append_darwin_oso_prefix(inv, cmd);

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
