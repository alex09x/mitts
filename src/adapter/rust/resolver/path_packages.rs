/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Path packages in the lock a manifest resolves to.

use super::CargoLockPackage;

/// Drops from `packages` the entries without a source that name a path package of the
/// workspace (`path_deps`: name and version), because those are added once, from their own
/// manifests, afterwards.
///
/// A `[patch]` that points at a path package puts it into the resolved graph, and when that
/// package is a workspace member or another path dependency (serde patches crates-io with its
/// own members) it would be listed twice. It is one package, as in Cargo; two entries of one
/// name and version without a source cannot be told apart when the lock is read back.
pub(super) fn drop_listed_twice(
    packages: &mut Vec<CargoLockPackage>,
    path_deps: &[(String, String)],
) {
    packages.retain(|p| {
        p.source.is_some()
            || !path_deps
                .iter()
                .any(|(name, version)| *name == p.name && *version == p.version)
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package(name: &str, version: &str, source: Option<&str>) -> CargoLockPackage {
        CargoLockPackage {
            name: name.into(),
            version: version.into(),
            source: source.map(str::to_owned),
            checksum: None,
            dependencies: Vec::new(),
        }
    }

    fn listed(packages: &[CargoLockPackage]) -> Vec<String> {
        packages
            .iter()
            .map(|p| format!("{} {} {}", p.name, p.version, p.source.is_some()))
            .collect()
    }

    const CRATES_IO: &str = "registry+https://github.com/rust-lang/crates.io-index";

    #[test]
    fn a_patched_member_is_dropped_so_the_member_entry_is_the_only_one() {
        let mut packages = vec![
            package("serde", "1.0.229", None),
            package("serde_json", "1.0.140", Some(CRATES_IO)),
        ];
        let members = vec![("serde".to_string(), "1.0.229".to_string())];
        drop_listed_twice(&mut packages, &members);
        assert_eq!(listed(&packages), ["serde_json 1.0.140 true"]);
    }

    #[test]
    fn a_registry_package_of_the_same_name_and_version_stays() {
        let mut packages = vec![
            package("serde", "1.0.229", None),
            package("serde", "1.0.229", Some(CRATES_IO)),
        ];
        let members = vec![("serde".to_string(), "1.0.229".to_string())];
        drop_listed_twice(&mut packages, &members);
        assert_eq!(listed(&packages), ["serde 1.0.229 true"]);
    }

    #[test]
    fn a_path_package_that_is_not_in_the_workspace_stays() {
        let mut packages = vec![
            package("shim", "0.1.0", None),
            package("shim", "0.2.0", None),
        ];
        let members = vec![("shim".to_string(), "0.1.0".to_string())];
        drop_listed_twice(&mut packages, &members);
        assert_eq!(listed(&packages), ["shim 0.2.0 false"]);
    }
}
