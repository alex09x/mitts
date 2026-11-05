/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Dependency entries of the path packages (workspace members) in a lock being written.
//!
//! Cargo writes a dependency as "name" when one package of that name is in the lock and as
//! "name version" when several are. A lock with a bare name for a name several packages share
//! cannot be read back: the entry is ambiguous.

use super::CargoLockPackage;

/// One dependency of a path package, before the lock is complete.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct DepSpec {
    pub name: String,
    /// The version of a path package dependency.
    pub path_version: Option<String>,
    /// The version requirement of a registry dependency.
    pub req: Option<String>,
}

impl DepSpec {
    pub(super) fn bare(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            path_version: None,
            req: None,
        }
    }

    pub(super) fn path(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            path_version: Some(version.into()),
            req: None,
        }
    }

    pub(super) fn registry(name: impl Into<String>, req: Option<String>) -> Self {
        Self {
            name: name.into(),
            path_version: None,
            req,
        }
    }

    /// The entry as written in the lock; `packages_of_name` is how many packages of this name
    /// the lock holds.
    pub(super) fn lock_entry(
        &self,
        packages: &[CargoLockPackage],
        packages_of_name: usize,
    ) -> String {
        if packages_of_name <= 1 {
            return self.name.clone();
        }
        if let Some(version) = &self.path_version {
            return format!("{} {version}", self.name);
        }
        let Some(req) = self
            .req
            .as_deref()
            .and_then(|r| semver::VersionReq::parse(r).ok())
        else {
            return self.name.clone();
        };
        // The newest registry or git version the requirement accepts, as the resolver picks.
        let chosen = packages
            .iter()
            .filter(|p| p.name == self.name && p.source.is_some())
            .filter_map(|p| {
                semver::Version::parse(&p.version)
                    .ok()
                    .filter(|v| req.matches(v))
            })
            .max();
        match chosen {
            Some(version) => format!("{} {version}", self.name),
            None => self.name.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CRATES_IO: &str = "registry+https://github.com/rust-lang/crates.io-index";

    fn package(name: &str, version: &str, source: Option<&str>) -> CargoLockPackage {
        CargoLockPackage {
            name: name.into(),
            version: version.into(),
            source: source.map(str::to_owned),
            checksum: None,
            dependencies: Vec::new(),
        }
    }

    fn syn_two_versions() -> Vec<CargoLockPackage> {
        vec![
            package("syn", "2.0.119", Some(CRATES_IO)),
            package("syn", "3.0.1", Some(CRATES_IO)),
        ]
    }

    #[test]
    fn a_name_held_once_stays_bare() {
        let packages = vec![package("quote", "1.0.40", Some(CRATES_IO))];
        let spec = DepSpec::registry("quote", Some("1.0.35".into()));
        assert_eq!(spec.lock_entry(&packages, 1), "quote");
    }

    #[test]
    fn a_registry_dependency_gets_the_version_its_requirement_picks() {
        let packages = syn_two_versions();
        let major_3 = DepSpec::registry("syn", Some("3".into()));
        assert_eq!(major_3.lock_entry(&packages, 2), "syn 3.0.1");
        let major_2 = DepSpec::registry("syn", Some("2.0.100".into()));
        assert_eq!(major_2.lock_entry(&packages, 2), "syn 2.0.119");
    }

    #[test]
    fn a_requirement_nothing_satisfies_or_none_keeps_the_bare_name() {
        let packages = syn_two_versions();
        assert_eq!(
            DepSpec::registry("syn", Some("9".into())).lock_entry(&packages, 2),
            "syn"
        );
        assert_eq!(
            DepSpec::registry("syn", None).lock_entry(&packages, 2),
            "syn"
        );
        assert_eq!(
            DepSpec::registry("syn", Some("not a requirement".into())).lock_entry(&packages, 2),
            "syn"
        );
    }

    #[test]
    fn a_path_dependency_carries_its_own_version() {
        let packages = vec![
            package("shim", "0.1.0", None),
            package("shim", "0.2.0", None),
        ];
        let spec = DepSpec::path("shim", "0.2.0");
        assert_eq!(spec.lock_entry(&packages, 2), "shim 0.2.0");
        assert_eq!(spec.lock_entry(&packages, 1), "shim");
    }

    #[test]
    fn a_path_package_of_the_same_name_is_not_a_registry_candidate() {
        let packages = vec![
            package("syn", "9.9.9", None),
            package("syn", "2.0.119", Some(CRATES_IO)),
        ];
        let spec = DepSpec::registry("syn", Some("*".into()));
        assert_eq!(spec.lock_entry(&packages, 2), "syn 2.0.119");
    }
}
