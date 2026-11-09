/*
 * Mitts Build — path dependency discovery
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::manifest::CargoToml;
use anyhow::{Context, Result};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

/// Removes `.` and resolves `..` against the preceding component without touching the
/// filesystem (symbolic links are kept as written), as Cargo does for path dependencies.
pub(super) fn normalize_lexically(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                let can_pop = matches!(out.components().next_back(), Some(Component::Normal(_)));
                if can_pop {
                    out.pop();
                } else if !matches!(
                    out.components().next_back(),
                    Some(Component::RootDir | Component::Prefix(_))
                ) {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() {
        out.push(".");
    }
    out
}

/// The values of a map in key order.
fn sorted_values<V>(map: &HashMap<String, V>) -> Vec<&V> {
    let mut entries: Vec<_> = map.iter().collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));
    entries.into_iter().map(|(_, v)| v).collect()
}

pub(super) fn discover_path_dependencies(
    project_dir: &Path,
    root_toml: &CargoToml,
    workspace_manifests: &HashMap<String, (PathBuf, CargoToml)>,
) -> Result<HashMap<String, Vec<(PathBuf, CargoToml)>>> {
    let mut path_dependencies: HashMap<String, Vec<(PathBuf, CargoToml)>> = HashMap::new();
    let mut seen_dirs: HashSet<PathBuf> = HashSet::new();
    let mut scan_deps = |base_dir: &Path,
                         deps: &HashMap<String, super::manifest::TomlDependency>,
                         pending: &mut VecDeque<(PathBuf, CargoToml)>|
     -> Result<()> {
        // Sorted: which spelling of a directory is met first must not depend on map order.
        let mut sorted: Vec<_> = deps.iter().collect();
        sorted.sort_by(|a, b| a.0.cmp(b.0));
        for (dep_name, dep_val) in sorted {
            if let super::manifest::TomlDependency::Detailed(ref d) = dep_val {
                if let Some(ref rel_p) = d.path {
                    // The same package is reached through different `path = "../.."` spellings
                    // from different dependents. Unit sources and keys are derived from this
                    // directory, so it must not depend on which dependent was scanned first.
                    let dep_dir = normalize_lexically(&base_dir.join(rel_p));
                    let canon = dep_dir.canonicalize().unwrap_or_else(|_| dep_dir.clone());
                    if !seen_dirs.insert(canon) {
                        continue;
                    }
                    let dep_manifest = dep_dir.join("Cargo.toml");
                    let dep_toml = CargoToml::from_file(&dep_manifest)
                        .with_context(|| format!("failed to read `{}`", dep_manifest.display()))?;
                    let name = dep_toml
                        .package
                        .as_ref()
                        .map(|p| p.name.clone())
                        .unwrap_or_else(|| dep_name.clone());
                    path_dependencies
                        .entry(name)
                        .or_default()
                        .push((dep_dir.clone(), dep_toml.clone()));
                    pending.push_back((dep_dir, dep_toml));
                }
            }
        }
        Ok(())
    };
    let mut pending: VecDeque<(PathBuf, CargoToml)> = VecDeque::new();
    if let Some(ref ws) = root_toml.workspace {
        scan_deps(project_dir, &ws.dependencies, &mut pending)?;
    }
    let mut members: Vec<_> = workspace_manifests.iter().collect();
    members.sort_by(|a, b| a.0.cmp(b.0));
    let tomls_to_scan: Vec<(&Path, &CargoToml)> = std::iter::once((project_dir, root_toml))
        .chain(members.into_iter().map(|(_, (p, t))| (p.as_path(), t)))
        .collect();
    // Lockfiles include workspace dev-dependencies even for builds that do not use them.
    for (base, toml) in tomls_to_scan {
        scan_deps(base, &toml.dependencies, &mut pending)?;
        scan_deps(base, &toml.build_dependencies, &mut pending)?;
        scan_deps(base, &toml.dev_dependencies, &mut pending)?;
        for t in sorted_values(&toml.target) {
            scan_deps(base, &t.dependencies, &mut pending)?;
            scan_deps(base, &t.build_dependencies, &mut pending)?;
            scan_deps(base, &t.dev_dependencies, &mut pending)?;
        }
    }
    for table in sorted_values(&root_toml.patch) {
        scan_deps(project_dir, table, &mut pending)?;
    }
    while let Some((dir, toml)) = pending.pop_front() {
        scan_deps(&dir, &toml.dependencies, &mut pending)?;
        scan_deps(&dir, &toml.build_dependencies, &mut pending)?;
        for t in sorted_values(&toml.target) {
            scan_deps(&dir, &t.dependencies, &mut pending)?;
            scan_deps(&dir, &t.build_dependencies, &mut pending)?;
        }
    }
    Ok(path_dependencies)
}

#[cfg(test)]
mod tests {
    use super::normalize_lexically;
    use std::path::{Path, PathBuf};

    #[test]
    fn parent_components_are_resolved_without_the_filesystem() {
        for (input, expected) in [
            ("/ws/crates/a/macros/../../b/logic", "/ws/crates/b/logic"),
            ("/ws/crates/a/./../b", "/ws/crates/b"),
            ("crates/a/../b", "crates/b"),
            ("../shared/../other", "../other"),
            ("a/..", "."),
            ("/../x", "/x"),
        ] {
            assert_eq!(
                normalize_lexically(Path::new(input)),
                PathBuf::from(expected),
                "{input}"
            );
        }
    }
}
