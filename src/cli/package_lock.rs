/*
 * Mitts Build — validate package lockfile inputs
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::adapter::rust::dependency_check::parse_package_spec;
use crate::adapter::rust::lockfile::CargoLock;
use crate::adapter::rust::manifest::{CargoToml, TomlDependency};
use anyhow::Result;
use std::path::Path;

pub fn compatible(lock: &CargoLock, manifest: &Path, name: &str, version: &str) -> Result<bool> {
    let roots: Vec<_> = lock
        .packages
        .iter()
        .filter(|p| p.name == name && p.version == version && p.source.is_none())
        .collect();
    if roots.len() != 1 || !lock.validate_manifest_compatibility(manifest)? {
        return Ok(false);
    }
    let root = roots[0];
    let toml = CargoToml::from_file(manifest)?;
    let mut maps = vec![
        &toml.dependencies,
        &toml.dev_dependencies,
        &toml.build_dependencies,
    ];
    for target in toml.target.values() {
        maps.extend([
            &target.dependencies,
            &target.dev_dependencies,
            &target.build_dependencies,
        ]);
    }
    for (alias, dependency) in maps.into_iter().flatten() {
        let (actual, requirement, path, git) = match dependency {
            TomlDependency::Simple(req) => (alias.as_str(), Some(req.as_str()), false, None),
            TomlDependency::Detailed(dep) => (
                dep.package_name.as_deref().unwrap_or(alias),
                dep.version.as_deref(),
                dep.path.is_some(),
                dep.git.as_deref(),
            ),
        };
        let requirement = requirement.map(semver::VersionReq::parse).transpose()?;
        let present = root.dependencies.iter().any(|spec| {
            let (dep_name, dep_version, dep_source) = parse_package_spec(spec);
            dep_name == actual
                && lock.packages.iter().any(|candidate| {
                    candidate.name == actual
                        && dep_version.is_none_or(|value| value == candidate.version)
                        && dep_source.is_none_or(|value| candidate.source.as_deref() == Some(value))
                        && requirement.as_ref().is_none_or(|req| {
                            semver::Version::parse(&candidate.version)
                                .is_ok_and(|version| req.matches(&version))
                        })
                        && if path {
                            candidate.source.is_none()
                        } else if let Some(url) = git {
                            candidate
                                .source
                                .as_deref()
                                .is_some_and(|source| source.starts_with(&format!("git+{url}")))
                        } else {
                            candidate
                                .source
                                .as_deref()
                                .is_some_and(|source| source.starts_with("registry+"))
                        }
                })
        });
        if !present {
            return Ok(false);
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::compatible;
    use crate::adapter::rust::lockfile::CargoLock;

    #[test]
    fn validates_renamed_dependencies_with_two_versions() {
        let temp = tempfile::tempdir().unwrap();
        let manifest = temp.path().join("Cargo.toml");
        std::fs::write(&manifest, "[package]\nname='root'\nversion='0.1.0'\n[dependencies]\nold={package='rand',version='0.8'}\nnew={package='rand',version='0.9'}\n").unwrap();
        let text = "version = 4\n\n[[package]]\nname = \"root\"\nversion = \"0.1.0\"\ndependencies = [\n \"rand 0.8.5\",\n \"rand 0.9.1\",\n]\n\n[[package]]\nname = \"rand\"\nversion = \"0.8.5\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\n\n[[package]]\nname = \"rand\"\nversion = \"0.9.1\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\n";
        let lock = CargoLock::parse(text).unwrap();
        assert!(compatible(&lock, &manifest, "root", "0.1.0").unwrap());
        let missing_edge = CargoLock::parse(&text.replace(" \"rand 0.9.1\",\n", "")).unwrap();
        assert!(!compatible(&missing_edge, &manifest, "root", "0.1.0").unwrap());
    }
}
