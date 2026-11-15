/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Which libraries only feed LTO, and so need bitcode but no object code.
//!
//! Under `lto = "fat"` or `"thin"` the final link re-optimizes and re-generates every Rust
//! library from its bitcode; the object code in an rlib is never used. Cargo knows this and
//! builds such a library with `-C linker-plugin-lto`, which skips instruction selection and
//! machine-code emission for it. Skipping that matters: on some targets it is most of the
//! compile (uroman on riscv64 spends minutes there, against seconds for its bitcode).
//!
//! The rule follows Cargo's `core/compiler/lto.rs`: a plain rlib is bitcode-only when LTO is
//! on and every unit that consumes it is either an LTO link (a binary, cdylib or staticlib)
//! or a bitcode-only rlib itself. A library nothing in this build consumes (a selected
//! package's lib) starts bitcode-only too, as in Cargo. Anything else that consumes it — a
//! dylib, a mixed `lib,cdylib`, a link without LTO — needs object code, and so does every
//! library below it on that path.

use super::types::PlannedCompilationUnit;
use std::collections::HashMap;
use std::path::Path;

/// Sets `lto_bitcode_only` on the units of a `build` plan. Only `build` uses it: test,
/// check and doc plans keep object code, as their consumers (test harnesses, rustdoc) link
/// without LTO.
pub(crate) fn assign(
    units: &mut [PlannedCompilationUnit],
    release: bool,
    ws_root: Option<&Path>,
) -> anyhow::Result<()> {
    let mut lto_on = Vec::with_capacity(units.len());
    for unit in units.iter() {
        let on = !unit.for_host && {
            let profile = super::profile::resolve_unit_profile_for(
                &unit.manifest_path,
                &unit.package_name,
                unit.is_workspace,
                unit.for_host,
                release,
                false,
                unit.workspace_root.as_deref().or(ws_root),
            )?;
            matches!(profile.lto.as_deref(), Some("fat" | "thin"))
        };
        lto_on.push(on);
    }
    let bitcode_only = bitcode_only_units(units, &lto_on);
    for (unit, only) in units.iter_mut().zip(bitcode_only) {
        unit.lto_bitcode_only = only;
    }
    Ok(())
}

/// `lto_on[i]` says whether unit `i` builds under a profile with LTO. Returns, per unit,
/// whether it needs only bitcode.
fn bitcode_only_units(units: &[PlannedCompilationUnit], lto_on: &[bool]) -> Vec<bool> {
    let index: HashMap<String, usize> = units
        .iter()
        .enumerate()
        .map(|(i, u)| (u.unit_id(), i))
        .collect();
    let mut consumers: Vec<Vec<usize>> = vec![Vec::new(); units.len()];
    for (i, unit) in units.iter().enumerate() {
        for dep in &unit.dependencies {
            if let Some(&d) = index.get(dep) {
                consumers[d].push(i);
            }
        }
    }
    let mut only: Vec<bool> = units
        .iter()
        .zip(lto_on)
        .map(|(u, &on)| on && !u.for_host && is_plain_rlib(&u.kind))
        .collect();
    // Dropping a library to object code can drop the libraries it consumes, so repeat until
    // nothing changes; each pass only ever clears flags, so this ends.
    let mut changed = true;
    while changed {
        changed = false;
        for r in 0..units.len() {
            if !only[r] {
                continue;
            }
            let all_bitcode = consumers[r].iter().all(|&c| {
                only[c] || (lto_on[c] && !units[c].for_host && is_lto_link(&units[c].kind))
            });
            if !all_bitcode {
                only[r] = false;
                changed = true;
            }
        }
    }
    only
}

/// A Rust library and nothing else; `lib,cdylib` also emits native code and is not.
fn is_plain_rlib(kind: &str) -> bool {
    kind.split(',')
        .map(str::trim)
        .all(|k| matches!(k, "lib" | "rlib"))
}

/// A link rustc runs LTO for: every output an executable, cdylib or staticlib. This matches
/// where `compiler_args` passes `-C lto`.
fn is_lto_link(kind: &str) -> bool {
    kind.split(',').map(str::trim).all(|k| {
        matches!(
            k,
            "bin" | "example" | "bench" | "test" | "custom-test" | "cdylib" | "staticlib"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// The target names of the flagged units.
    fn ids(units: &[PlannedCompilationUnit], flags: &[bool]) -> HashSet<String> {
        units
            .iter()
            .zip(flags)
            .filter(|(_, &f)| f)
            .map(|(u, _)| u.target_name.clone())
            .collect()
    }

    fn unit(name: &str, kind: &str, deps: &[&PlannedCompilationUnit]) -> PlannedCompilationUnit {
        PlannedCompilationUnit {
            package_id: format!("{name}@1.0.0"),
            package_name: name.to_string(),
            version: "1.0.0".to_string(),
            target_name: name.to_string(),
            kind: kind.to_string(),
            dependencies: deps.iter().map(|d| d.unit_id()).collect(),
            ..Default::default()
        }
    }

    fn set(names: &[&str]) -> HashSet<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn libraries_under_an_lto_binary_need_only_bitcode() {
        // The Melodia shape: uroman under a workspace lib under the binary.
        let uroman = unit("uroman", "lib", &[]);
        let app = unit("app", "lib", &[&uroman]);
        let bin = unit("melodia", "bin", &[&app]);
        let units = vec![uroman, app, bin];
        let only = bitcode_only_units(&units, &[true; 3]);
        assert_eq!(ids(&units, &only), set(&["uroman", "app"]));
    }

    #[test]
    fn a_selected_library_with_no_consumer_starts_bitcode_only() {
        let dep = unit("dep", "lib", &[]);
        let lib = unit("lib", "lib", &[&dep]);
        let units = vec![dep, lib];
        let only = bitcode_only_units(&units, &[true; 2]);
        assert_eq!(ids(&units, &only), set(&["dep", "lib"]));
    }

    #[test]
    fn no_lto_means_object_code_everywhere() {
        let dep = unit("dep", "lib", &[]);
        let bin = unit("bin", "bin", &[&dep]);
        let units = vec![dep, bin];
        assert!(ids(&units, &bitcode_only_units(&units, &[false; 2])).is_empty());
    }

    #[test]
    fn a_consumer_that_needs_object_code_keeps_it_all_the_way_down() {
        // dep <- mid <- dylib: the dylib links without LTO, so mid and dep need objects even
        // though dep also feeds an LTO binary directly.
        let dep = unit("dep", "lib", &[]);
        let mid = unit("mid", "lib", &[&dep]);
        let dylib = unit("plugin", "dylib", &[&mid]);
        let bin = unit("bin", "bin", &[&dep]);
        let units = vec![dep, mid, dylib, bin];
        assert!(ids(&units, &bitcode_only_units(&units, &[true; 4])).is_empty());
    }

    #[test]
    fn a_mixed_native_library_needs_object_code_from_its_dependencies() {
        let dep = unit("dep", "lib", &[]);
        let mixed = unit("ffi", "lib,cdylib", &[&dep]);
        let units = vec![dep, mixed];
        assert!(ids(&units, &bitcode_only_units(&units, &[true; 2])).is_empty());
    }

    #[test]
    fn cdylib_and_staticlib_links_run_lto() {
        let dep = unit("dep", "lib", &[]);
        let cdylib = unit("ffi", "cdylib", &[&dep]);
        let staticlib = unit("st", "staticlib", &[&dep]);
        let units = vec![dep, cdylib, staticlib];
        assert_eq!(
            ids(&units, &bitcode_only_units(&units, &[true; 3])),
            set(&["dep"])
        );
    }

    #[test]
    fn host_units_never_go_bitcode_only() {
        let mut dep = unit("dep", "lib", &[]);
        dep.for_host = true;
        let mut pm = unit("derive", "proc-macro", &[&dep]);
        pm.for_host = true;
        let units = vec![dep, pm];
        assert!(ids(&units, &bitcode_only_units(&units, &[true; 2])).is_empty());
    }
}
