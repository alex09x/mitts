/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Key for the cross-process single-flight lock around a unit compile.

use super::types::PlannedCompilationUnit;
use crate::core::hash::Sha256;
use std::path::Path;

/// Units that share a coarse action key can still read different modules. Those builds
/// produce different results, so they must not queue behind each other on one lock.
/// The lock only dedups identical work and never decides a hit, so the key adds the
/// crate root's directory tree: the exact dep-info set is unknown before rustc runs.
pub fn single_flight_key(unit: &PlannedCompilationUnit, action_key: &str) -> String {
    if !super::unit_inputs::is_mutable_unit(unit) {
        return action_key.to_string();
    }
    let pkg_dir = unit.manifest_path.parent().unwrap_or(Path::new("."));
    let Some(dir) = unit
        .src_path
        .parent()
        .filter(|d| *d != pkg_dir && d.starts_with(pkg_dir))
    else {
        return action_key.to_string();
    };
    let mut files = Vec::new();
    if super::unit_inputs::scan_dir_inputs(dir, pkg_dir, &mut files).is_err() {
        return action_key.to_string();
    }
    files.sort();
    let mut payload = format!("single_flight:{action_key}\n");
    for (path, digest) in files {
        payload.push_str(&format!("{path}={digest}\n"));
    }
    Sha256::digest(payload.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn unit_in(dir: &Path) -> PlannedCompilationUnit {
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname = \"sf\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        fs::write(dir.join("src/lib.rs"), "mod m;\n").unwrap();
        PlannedCompilationUnit {
            package_name: "sf".into(),
            target_name: "sf".into(),
            kind: "lib".into(),
            src_path: dir.join("src/lib.rs"),
            manifest_path: dir.join("Cargo.toml"),
            is_workspace: true,
            ..Default::default()
        }
    }

    #[test]
    fn same_coarse_key_with_different_modules_takes_different_locks() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let c = tempfile::tempdir().unwrap();
        let (ua, ub, uc) = (unit_in(a.path()), unit_in(b.path()), unit_in(c.path()));
        fs::write(a.path().join("src/m.rs"), "pub fn v() -> u32 { 1 }\n").unwrap();
        fs::write(b.path().join("src/m.rs"), "pub fn v() -> u32 { 2 }\n").unwrap();
        fs::write(c.path().join("src/m.rs"), "pub fn v() -> u32 { 1 }\n").unwrap();
        fs::write(c.path().join("README.md"), "other\n").unwrap();

        let ka = single_flight_key(&ua, "coarse");
        assert_ne!(ka, single_flight_key(&ub, "coarse"));
        assert_eq!(
            ka,
            single_flight_key(&uc, "coarse"),
            "identical crate sources in another checkout must share the lock"
        );
    }
}
