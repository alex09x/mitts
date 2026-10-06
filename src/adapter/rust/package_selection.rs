/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::adapter::rust::lockfile::{CargoLock, CargoLockPackage};
use crate::adapter::rust::manifest::CargoToml;
use crate::adapter::rust::types::RustBuildOptions;
use anyhow::{bail, Result};
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// Selects root packages and performs graph traversal to collect all reachable packages.
pub fn select_transitive_packages<'a>(
    lockfile: &'a CargoLock,
    workspace_manifests: &HashMap<String, (std::path::PathBuf, CargoToml)>,
    root_toml: &CargoToml,
    project_dir: &Path,
    opt: &RustBuildOptions,
) -> Result<Vec<(&'a CargoLockPackage, bool)>> {
    let mut packages_by_name: HashMap<String, Vec<&'a CargoLockPackage>> = HashMap::new();
    for pkg in &lockfile.packages {
        packages_by_name
            .entry(pkg.name.clone())
            .or_default()
            .push(pkg);
    }

    // Determine root packages to compile
    let mut root_pkgs: Vec<&'a CargoLockPackage> = Vec::new();
    if let Some(ref target_pkg) = opt.package {
        if let Some(pkgs) = packages_by_name.get(target_pkg) {
            root_pkgs.extend(pkgs);
        } else {
            bail!("Package '{}' not found in workspace lockfile", target_pkg);
        }
    } else {
        let default_members = root_toml
            .workspace
            .as_ref()
            .map(|w| &w.default_members)
            .filter(|dm| !dm.is_empty());

        if let Some(dm) = default_members {
            for member_pat in dm {
                let member_dir = if member_pat == "." {
                    project_dir.to_path_buf()
                } else {
                    project_dir.join(member_pat)
                };
                for (name, (dir, _)) in workspace_manifests {
                    if dir == &member_dir || (member_pat == "." && dir == project_dir) {
                        if let Some(pkgs) = packages_by_name.get(name) {
                            root_pkgs.extend(pkgs);
                        }
                    }
                }
            }
        }

        if root_pkgs.is_empty() {
            for name in workspace_manifests.keys() {
                if let Some(pkgs) = packages_by_name.get(name) {
                    root_pkgs.extend(pkgs);
                }
            }
        }
    }

    // Graph traversal: collect all reachable packages
    let mut selected_packages: Vec<(&'a CargoLockPackage, bool)> = Vec::new();
    let mut visited: HashSet<(String, String)> = HashSet::new();
    let mut to_visit: Vec<&'a CargoLockPackage> = root_pkgs;

    while let Some(pkg) = to_visit.pop() {
        let key = (pkg.name.clone(), pkg.version.clone());
        if visited.insert(key) {
            let is_ws = workspace_manifests.contains_key(&pkg.name);
            selected_packages.push((pkg, is_ws));

            for dep_spec in &pkg.dependencies {
                let (dep_name, dep_ver) = parse_dep_spec(dep_spec);
                if is_target_incompatible_with_host(dep_name) {
                    continue;
                }
                if let Some(candidates) = packages_by_name.get(dep_name) {
                    if let Some(ver) = dep_ver {
                        if let Some(matching) = candidates.iter().find(|p| p.version == ver) {
                            if !visited.contains(&(matching.name.clone(), matching.version.clone()))
                            {
                                to_visit.push(matching);
                            }
                        }
                    } else if let Some(matching) = candidates.first() {
                        if !visited.contains(&(matching.name.clone(), matching.version.clone())) {
                            to_visit.push(matching);
                        }
                    }
                }
            }
        }
    }

    Ok(selected_packages)
}

/// Checks whether a package is strictly exclusive to a foreign target platform.
pub fn is_target_incompatible_with_host(dep_name: &str) -> bool {
    #[cfg(not(target_os = "android"))]
    if matches!(
        dep_name,
        "ndk-sys" | "ndk" | "android-activity" | "android_system_properties"
    ) {
        return true;
    }
    #[cfg(not(target_os = "redox"))]
    if matches!(dep_name, "orbclient") {
        return true;
    }
    #[cfg(not(target_arch = "wasm32"))]
    if matches!(dep_name, "web-sys" | "js-sys" | "wasm-bindgen-futures") {
        return true;
    }
    #[cfg(not(target_os = "windows"))]
    {
        if matches!(
            dep_name,
            "glutin_wgl_sys"
                | "accesskit_windows"
                | "windows"
                | "windows-future"
                | "windows-collections"
                | "windows-numerics"
                | "windows-threading"
                | "schannel"
        ) || dep_name.starts_with("windows_")
        {
            return true;
        }
    }
    #[cfg(not(any(
        target_os = "linux",
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd"
    )))]
    {
        if matches!(
            dep_name,
            "accesskit_unix"
                | "accesskit_atspi_common"
                | "glutin_glx_sys"
                | "calloop-wayland-source"
                | "sctk-adwaita"
                | "smithay-client-toolkit"
                | "smithay-clipboard"
                | "xkbcommon-dl"
                | "x11-dl"
                | "x11rb"
                | "x11rb-protocol"
        ) || dep_name.starts_with("wayland-")
            || dep_name.starts_with("smithay-")
        {
            return true;
        }
    }
    false
}

/// Parses a dependency specification from Cargo.lock into name and optional version.
pub fn parse_dep_spec(spec: &str) -> (&str, Option<&str>) {
    let mut parts = spec.split_whitespace();
    let name = parts.next().unwrap_or(spec);
    let version = parts.next();
    (name, version)
}
