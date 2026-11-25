/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::metadata_deps::package_dependencies;
use super::metadata_inherit::Patches;
use crate::adapter::rust::manifest::{CargoToml, DiscoveredTarget, TestRole, TomlDependency};
use crate::adapter::rust::registry_config::RegistryConfig;
use crate::adapter::rust::{RustAdapter, RustBuildOptions};
use crate::cli::MetadataArgs;
use anstream::{eprintln, println};
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

struct PackageEntry {
    cargo_id: String,
    name: String,
    version: String,
    source: Option<String>,
    manifest_path: PathBuf,
    manifest: CargoToml,
    is_workspace: bool,
    features: BTreeSet<String>,
}

pub fn handle_metadata(args: &MetadataArgs) -> i32 {
    match metadata_json(args) {
        Ok(value) => match serde_json::to_string_pretty(&value) {
            Ok(output) => {
                println!("{output}");
                0
            }
            Err(err) => {
                eprintln!("mitts metadata: failed to serialize JSON: {err}");
                1
            }
        },
        Err(err) => {
            crate::cli::ui::Ui::report_error(&err);
            1
        }
    }
}

fn metadata_json(args: &MetadataArgs) -> Result<Value> {
    if args.format_version != 1 {
        anyhow::bail!(
            "unsupported metadata format version {}; supported version is 1",
            args.format_version
        );
    }
    let mut opt = RustBuildOptions {
        manifest_path: crate::cli::manifest_discovery::discover_manifest(&args.manifest_path),
        package: args.package.clone(),
        features: args.features.clone(),
        no_default_features: args.no_default_features,
        all_features: args.all_features,
        cache_dir: args.cache_dir.clone(),
        target_triple: args.filter_platform.clone(),
        locked: args.locked || args.frozen,
        offline: args.offline || args.frozen,
        frozen: args.frozen,
        ..Default::default()
    };
    crate::cli::manifest_discovery::select_workspace_member(&mut opt, true);
    if args.no_deps {
        return no_deps_json(&opt);
    }
    metadata_for(&opt, args.package.clone(), args.filter_platform.is_none())
}

/// `metadata --no-deps`: the workspace members from their manifests alone, without
/// resolving or fetching any dependency, and no resolve graph, as Cargo prints it.
fn no_deps_json(opt: &RustBuildOptions) -> Result<Value> {
    let start = super::project_info::starting_manifest(&opt.manifest_path)?;
    let root = super::project_info::workspace_root(&start)?;
    let root_manifest = CargoToml::from_file(&root)?;
    let registries = RegistryConfig::load(root.parent().unwrap_or(Path::new(".")))?;
    let mut packages = Vec::new();
    let mut members = Vec::new();
    for (_, dir) in super::project_info::local_package_dirs(&root)? {
        let manifest_path = dir.join("Cargo.toml");
        let manifest = CargoToml::from_file(&manifest_path).with_context(|| {
            format!(
                "failed to parse package manifest {}",
                manifest_path.display()
            )
        })?;
        let Some(package) = manifest.package.clone() else {
            continue;
        };
        let version = package
            .version
            .clone()
            .unwrap_or_else(|| "0.0.0".to_string());
        let canonical = dir.canonicalize().unwrap_or(dir.clone());
        let entry = PackageEntry {
            cargo_id: cargo_package_id("", &package.name, &version, &canonical.join("Cargo.toml")),
            name: package.name.clone(),
            version,
            source: None,
            manifest_path: manifest_path.clone(),
            manifest,
            is_workspace: true,
            features: BTreeSet::new(),
        };
        members.push(entry.cargo_id.clone());
        packages.push(package_json(&entry, &registries)?);
    }
    let workspace_root = root.parent().unwrap_or(Path::new(".")).to_path_buf();
    let target_directory = RustAdapter::resolve_target_dir_without_tags(&root, opt)
        .map(|t| {
            if t.is_absolute() {
                t
            } else {
                workspace_root.join(t)
            }
        })
        .unwrap_or_else(|_| workspace_root.join("target"));
    let defaults = root_manifest
        .workspace
        .as_ref()
        .map(|w| w.default_members.clone())
        .filter(|d| !d.is_empty());
    let default_members: Vec<String> = match defaults {
        Some(defaults) => members
            .iter()
            .zip(packages.iter())
            .filter(|(_, p)| {
                let dir = Path::new(p["manifest_path"].as_str().unwrap_or("")).parent();
                let rel = dir
                    .and_then(|d| d.strip_prefix(&workspace_root).ok())
                    .map(|d| d.to_string_lossy().trim_end_matches('/').to_string())
                    .unwrap_or_default();
                defaults.iter().any(|m| {
                    let m = m.trim_start_matches("./").trim_end_matches('/');
                    (m == "." && rel.is_empty()) || super::project_new::glob_match(m, &rel)
                })
            })
            .map(|(id, _)| id.clone())
            .collect(),
        // Without `default-members`: the root package of a non-virtual workspace, every
        // member of a virtual one, as Cargo does.
        None => match &root_manifest.package {
            Some(root_package) => members
                .iter()
                .zip(packages.iter())
                .filter(|(_, p)| p["name"].as_str() == Some(root_package.name.as_str()))
                .filter(|(_, p)| {
                    Path::new(p["manifest_path"].as_str().unwrap_or(""))
                        .parent()
                        .is_some_and(|d| d == workspace_root)
                })
                .map(|(id, _)| id.clone())
                .collect(),
            None => members.clone(),
        },
    };
    Ok(json!({
        "packages": packages,
        "workspace_members": members,
        "workspace_default_members": default_members,
        "resolve": null,
        "target_directory": target_directory,
        "workspace_root": workspace_root,
        "version": 1,
    }))
}

/// The `cargo metadata` document for a plan made with `opt` (`mitts tree` plans in test
/// mode, so that dev-dependencies are part of the graph).
/// `all_platforms`: the graph of every platform and every kind of dependency, as `cargo
/// metadata` reports it without `--filter-platform` (`tree` filters by the host).
pub(crate) fn metadata_for(
    opt: &RustBuildOptions,
    package: Option<String>,
    all_platforms: bool,
) -> Result<Value> {
    let mut opt = opt.clone();
    opt.inspect_only = true;
    let opt = &opt;
    let plan = RustAdapter::explain_plan(opt)?;
    let mut packages = BTreeMap::<String, PackageEntry>::new();
    let mut unit_packages = HashMap::<String, String>::new();

    for unit in &plan.units {
        let cargo_id = cargo_package_id(
            &unit.package_id,
            &unit.package_name,
            &unit.version,
            &unit.manifest_path,
        );
        unit_packages.insert(unit.unit_id(), unit.package_id.clone());
        if !packages.contains_key(&unit.package_id) {
            let manifest = CargoToml::from_file(&unit.manifest_path).with_context(|| {
                format!(
                    "failed to parse package manifest {}",
                    unit.manifest_path.display()
                )
            })?;
            let source = package_source(&unit.package_id);
            packages.insert(
                unit.package_id.clone(),
                PackageEntry {
                    cargo_id,
                    name: unit.package_name.clone(),
                    version: unit.version.clone(),
                    source,
                    manifest_path: unit.manifest_path.clone(),
                    manifest,
                    is_workspace: unit.is_workspace,
                    features: BTreeSet::new(),
                },
            );
        }
        let entry = packages.get_mut(&unit.package_id).unwrap();
        entry.is_workspace |= unit.is_workspace;
        entry.features.extend(unit.features.iter().cloned());
    }

    let package_id_by_unit_id = unit_packages;
    let cargo_ids_by_internal: HashMap<String, String> = packages
        .iter()
        .map(|(internal, entry)| (internal.clone(), entry.cargo_id.clone()))
        .collect();
    let package_names_by_id: HashMap<String, String> = packages
        .values()
        .map(|entry| (entry.cargo_id.clone(), entry.name.clone()))
        .collect();
    let package_versions_by_id: HashMap<String, String> = packages
        .values()
        .map(|entry| (entry.cargo_id.clone(), entry.version.clone()))
        .collect();
    let package_sources_by_id: HashMap<String, Option<String>> = packages
        .values()
        .map(|entry| (entry.cargo_id.clone(), entry.source.clone()))
        .collect();

    // Per package: its directory and its library's crate name (`[lib] name`, else the
    // package name), what path declarations and unrenamed extern names are matched by.
    let package_dirs_by_id: HashMap<String, (PathBuf, String)> = packages
        .values()
        .map(|entry| {
            let lib = entry
                .manifest
                .lib
                .as_ref()
                .and_then(|l| l.name.clone())
                .unwrap_or_else(|| entry.name.clone())
                .replace('-', "_");
            let dir = entry
                .manifest_path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_default();
            (entry.cargo_id.clone(), (dir, lib))
        })
        .collect();
    // The workspace root's `[patch]` tables (none when the manifest cannot be read here).
    // Lexically normalized as the planner normalizes it: `sub/../Cargo.toml` must not make
    // the config of `sub/` apply.
    let root_manifest_path = super::metadata_deps::lexical(&std::path::absolute(
        super::project_info::starting_manifest(&opt.manifest_path)?,
    )?);
    let patches = match CargoToml::from_file(&root_manifest_path) {
        Ok(root) => Patches::of(&root_manifest_path, &root),
        Err(_) => Patches::of(&root_manifest_path, &CargoToml::default()),
    };
    // `registry = "<name>"` declarations are matched to the index the name stands for, in
    // the configuration the resolver read: the one that applies to the workspace root,
    // where the lockfile is resolved.
    let registries = RegistryConfig::load(patches.root())?;
    // Without a platform filter Cargo's graph is the lockfile's: every dependency of every
    // platform and kind. The plan covers the host only, so the lock adds the rest.
    let lock = all_platforms
        .then(|| {
            super::project_info::workspace_root(&root_manifest_path)
                .ok()
                .and_then(|root| {
                    crate::adapter::rust::lockfile::CargoLock::from_file(
                        &root.with_file_name("Cargo.lock"),
                    )
                    .ok()
                })
        })
        .flatten();
    let id_by_name_version: HashMap<(String, String), String> = packages
        .values()
        .map(|e| ((e.name.clone(), e.version.clone()), e.cargo_id.clone()))
        .collect();
    let platform_filter = if all_platforms {
        None
    } else {
        Some(opt.target_triple.as_deref())
    };
    let mut nodes = Vec::new();
    for (internal_id, entry) in &packages {
        let mut outgoing = BTreeMap::<String, BTreeSet<String>>::new();
        if let Some(lock) = &lock {
            let locked = lock
                .packages
                .iter()
                .find(|p| p.name == entry.name && p.version == entry.version);
            for spec in locked
                .map(|p| p.dependencies.as_slice())
                .unwrap_or_default()
            {
                let (name, version, _) =
                    crate::adapter::rust::dependency_check::parse_package_spec(spec);
                let child_id = match version {
                    Some(v) => id_by_name_version.get(&(name.to_string(), v.to_string())),
                    None => packages
                        .values()
                        .find(|e| e.name == name)
                        .map(|e| &e.cargo_id),
                };
                let Some(child_id) = child_id else { continue };
                if child_id == &entry.cargo_id {
                    continue;
                }
                for kind in dependency_kinds(
                    &entry.manifest,
                    name,
                    None,
                    entry.manifest_path.parent().unwrap_or(Path::new(".")),
                    true,
                ) {
                    outgoing.entry(child_id.clone()).or_default().insert(kind);
                }
            }
        }
        for unit in plan
            .units
            .iter()
            .filter(|unit| &unit.package_id == internal_id)
        {
            for dependency in &unit.dependencies {
                let Some(child_internal_id) = package_id_by_unit_id.get(dependency) else {
                    continue;
                };
                let Some(child_id) = cargo_ids_by_internal.get(child_internal_id) else {
                    continue;
                };
                if child_id == &entry.cargo_id {
                    continue;
                }
                let child_name = package_names_by_id
                    .get(child_id)
                    .map(String::as_str)
                    .unwrap_or("");
                let kinds = dependency_kinds(
                    &entry.manifest,
                    child_name,
                    platform_filter.flatten(),
                    entry.manifest_path.parent().unwrap_or(Path::new(".")),
                    platform_filter.is_none(),
                );
                for kind in kinds {
                    outgoing.entry(child_id.clone()).or_default().insert(kind);
                }
            }
        }
        // One edge per extern crate name, as Cargo writes them: a package declared under two
        // keys (`old = { package = "rand" }` next to `rand`) is two edges, each with the kinds
        // of its own declarations.
        let deps = outgoing
            .into_iter()
            .flat_map(|(pkg, kinds)| {
                let package = package_names_by_id.get(&pkg).cloned().unwrap_or_default();
                let version = package_versions_by_id
                    .get(&pkg)
                    .cloned()
                    .unwrap_or_default();
                let active: Vec<String> = kinds.into_iter().collect();
                let source = package_sources_by_id.get(&pkg).cloned().flatten();
                let (dir, lib_name) = package_dirs_by_id.get(&pkg).cloned().unwrap_or_default();
                let child = EdgeTarget {
                    name: &package,
                    version: &version,
                    source: source.as_deref(),
                    dir: &dir,
                    lib_name: &lib_name,
                };
                declaration_edges(
                    &entry.manifest,
                    entry.manifest_path.parent().unwrap_or(Path::new(".")),
                    &patches,
                    &entry.features,
                    &registries,
                    &child,
                    &active,
                )
                .into_iter()
                .map(move |(name, edge_kinds)| {
                    let dep_kinds = edge_kinds
                        .iter()
                        .map(|kind| serde_json::from_str::<Value>(kind).unwrap_or(Value::Null))
                        .collect::<Vec<_>>();
                    json!({"name": name, "pkg": pkg.clone(), "dep_kinds": dep_kinds})
                })
                .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        let dependencies = deps
            .iter()
            .map(|dependency| dependency["pkg"].clone())
            .collect::<Vec<_>>();
        nodes.push(json!({
            "id": entry.cargo_id,
            "deps": deps,
            "dependencies": dependencies,
            "features": entry.features,
        }));
    }

    let package_values = packages
        .values()
        .map(|entry| package_json(entry, &registries))
        .collect::<Result<Vec<_>>>()?;
    // In Cargo's order: the root package and the listed members as declared, then the
    // members it adds itself (path dependencies inside the workspace) by name.
    let declared: Vec<String> = super::project_info::workspace_root(&plan.manifest_path)
        .and_then(|root| super::project_info::local_package_dirs(&root))
        .map(|dirs| dirs.into_iter().map(|(name, _)| name).collect())
        .unwrap_or_default();
    let mut members: Vec<&PackageEntry> = packages
        .values()
        .filter(|entry| entry.is_workspace)
        .collect();
    members.sort_by_key(|entry| {
        (
            declared
                .iter()
                .position(|n| *n == entry.name)
                .unwrap_or(usize::MAX),
            entry.name.clone(),
        )
    });
    let workspace_members = members
        .into_iter()
        .map(|entry| entry.cargo_id.clone())
        .collect::<Vec<_>>();
    let root_manifest = CargoToml::from_file(&plan.manifest_path)?;
    let root_name = package.or_else(|| {
        root_manifest
            .package
            .as_ref()
            .map(|package| package.name.clone())
    });
    let root = root_name.and_then(|name| {
        packages
            .values()
            .find(|entry| entry.name == name && entry.is_workspace)
            .map(|entry| entry.cargo_id.clone())
    });
    let workspace_root =
        crate::adapter::rust::compiler_resolve::find_workspace_root(&plan.manifest_path);
    let workspace_default_members = root_manifest
        .workspace
        .as_ref()
        .map(|workspace| workspace.default_members.clone())
        .filter(|members| !members.is_empty())
        .map(|members| {
            packages
                .values()
                .filter(|entry| entry.is_workspace)
                .filter(|entry| {
                    let relative = entry
                        .manifest_path
                        .parent()
                        .and_then(|path| path.strip_prefix(&workspace_root).ok())
                        .map(|path| path.to_string_lossy().trim_end_matches('/').to_string())
                        .unwrap_or_default();
                    members.iter().any(|member| {
                        let member = member.trim_start_matches("./").trim_end_matches('/');
                        member == entry.name
                            || member == relative
                            || (member == "." && relative.is_empty())
                    })
                })
                .map(|entry| entry.cargo_id.clone())
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| {
            if root_manifest.workspace.is_some() {
                workspace_members.clone()
            } else {
                root.clone().into_iter().collect()
            }
        });

    Ok(json!({
        "packages": package_values,
        "workspace_members": workspace_members,
        "workspace_default_members": workspace_default_members,
        "resolve": {"root": root, "nodes": nodes},
        "target_directory": plan.target_directory,
        "workspace_root": workspace_root,
        "version": 1,
    }))
}

fn package_json(entry: &PackageEntry, registries: &RegistryConfig) -> Result<Value> {
    let package = entry.manifest.package.as_ref();
    let package_dir = entry
        .manifest_path
        .parent()
        .context("package manifest has no parent directory")?;
    let mut targets = entry.manifest.discover_targets(package_dir);
    targets.extend(
        entry
            .manifest
            .discover_test_targets(package_dir)
            .into_iter()
            .filter(|target| target.test_role == Some(TestRole::Integration)),
    );
    let mut seen = BTreeSet::new();
    targets.retain(|target| {
        seen.insert((
            target.name.clone(),
            target.kind.clone(),
            target.src_path.clone(),
        ))
    });
    targets.sort_by(|left, right| {
        (
            left.name.as_str(),
            left.kind.as_str(),
            left.src_path.as_path(),
        )
            .cmp(&(
                right.name.as_str(),
                right.kind.as_str(),
                right.src_path.as_path(),
            ))
    });
    let mut target_values = targets.iter().map(target_json).collect::<Vec<_>>();
    // The build script is a target too, as Cargo lists it.
    if let Some(script) = crate::adapter::rust::build_script::resolve_build_script_path_with_toml(
        package_dir,
        &entry.manifest,
    ) {
        target_values.push(json!({
            "kind": ["custom-build"],
            "crate_types": ["bin"],
            "name": "build-script-build",
            "src_path": script,
            "edition": package.and_then(|p| p.edition.clone()).unwrap_or_else(|| "2015".to_string()),
            "doc": false,
            "doctest": false,
            "test": false,
        }));
    }
    let dependencies = package_dependencies(&entry.manifest, package_dir, registries);
    Ok(json!({
        "id": entry.cargo_id,
        "name": entry.name,
        "version": entry.version,
        "source": entry.source,
        "manifest_path": entry.manifest_path,
        "description": package.and_then(|p| p.description.clone()),
        "homepage": package.and_then(|p| p.homepage.clone()),
        "repository": package.and_then(|p| p.repository.clone()),
        "authors": package.and_then(|p| p.authors.clone()).unwrap_or_default(),
        "license": package.and_then(|p| p.license.clone()),
        "readme": package.and_then(|p| p.readme.clone()),
        "links": package.and_then(|p| p.links.clone()),
        "edition": package.and_then(|p| p.edition.clone()).unwrap_or_else(|| "2015".to_string()),
        "features": entry.manifest.features,
        "dependencies": dependencies,
        "targets": target_values,
        "metadata": Value::Null,
    }))
}

fn target_json(target: &DiscoveredTarget) -> Value {
    json!({
        "kind": [target.kind],
        "crate_types": target.crate_types,
        "name": target.name,
        "src_path": target.src_path,
        "edition": target.edition.as_deref().unwrap_or("2015"),
        "doc": target.kind == "lib" || target.kind == "proc-macro",
        "doctest": target.kind == "lib" || target.kind == "proc-macro",
        "test": target.kind != "example",
        "harness": target.harness,
        "required-features": target.required_features,
    })
}

/// Whether a non-path declaration is the one a resolved package (`source`: its lock source,
/// `None` for a path package) came from (path declarations are matched by directory in
/// `declaration_edges`): a git one the same
/// repository at the same reference (`?branch=`, `?tag=`, `?rev=` or none); a registry one
/// the exact index its registry name (crates.io when unnamed) stands for in `registries`.
fn same_source(dep: &TomlDependency, source: Option<&str>, registries: &RegistryConfig) -> bool {
    let detail = match dep {
        TomlDependency::Simple(_) => None,
        TomlDependency::Detailed(d) => Some(d),
    };
    let trim = |url: &str| {
        url.trim_end_matches('/')
            .trim_end_matches(".git")
            .to_string()
    };
    match (detail, source) {
        (Some(d), Some(s)) if d.git.is_some() => {
            let Some(rest) = s.strip_prefix("git+") else {
                return false;
            };
            let rest = rest.split('#').next().unwrap_or(rest);
            let (base, query) = rest.split_once('?').unwrap_or((rest, ""));
            let wanted = match (&d.branch, &d.tag, &d.rev) {
                (Some(b), _, _) => format!("branch={b}"),
                (_, Some(t), _) => format!("tag={t}"),
                (_, _, Some(r)) => format!("rev={r}"),
                _ => String::new(),
            };
            trim(base) == trim(d.git.as_deref().unwrap_or_default()) && query == wanted
        }
        (_, Some(s)) => {
            let registry = detail.and_then(|d| d.registry.as_deref());
            registries
                .lock_source_for_registry(registry)
                .is_ok_and(|expected| expected == s)
        }
        (_, None) => false,
    }
}

/// Whether the declaration under `key` is part of the build of a package with `features`
/// enabled: a required one always; an optional one when a feature enables it (its implicit
/// feature `key`, `dep:key`, or a non-weak `key/feature`), as Cargo activates it.
fn declaration_active(
    manifest: &CargoToml,
    key: &str,
    dep: &TomlDependency,
    features: &BTreeSet<String>,
) -> bool {
    let optional = matches!(dep, TomlDependency::Detailed(d) if d.optional == Some(true));
    if !optional {
        return true;
    }
    let explicit = format!("dep:{key}");
    let through = format!("{key}/");
    let named_explicitly = manifest.features.values().flatten().any(|v| *v == explicit);
    // Without `dep:key` anywhere the dependency has an implicit feature of its own name.
    if !named_explicitly && features.contains(key) {
        return true;
    }
    features.iter().any(|f| {
        manifest
            .features
            .get(f)
            .into_iter()
            .flatten()
            .any(|v| *v == explicit || v.starts_with(&through))
    })
}

/// The resolved package a dependency edge goes to, as declarations are matched against it.
struct EdgeTarget<'a> {
    name: &'a str,
    version: &'a str,
    /// Its lock source; `None` for a path package.
    source: Option<&'a str>,
    /// Its package directory (a path declaration must point there).
    dir: &'a Path,
    /// Its library's crate name: the extern name of an unrenamed declaration.
    lib_name: &'a str,
}

/// Whether two package directories are the same one (symlinks and `..` resolved where the
/// file system allows, else compared as written).
fn same_dir(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// The edges from `manifest`'s package (in `parent_dir`, built with `features`) to
/// `child`: one per extern crate name (the manifest key of a renamed declaration, else the
/// child's library name), each with the kinds, among the `active` ones, of the declarations
/// under that name that are part of the build, come from the child's source and whose
/// requirement admits its version.
///
/// `manifest` is normalized (`workspace = true` resolved, see `metadata_inherit`); a
/// registry declaration also matches the package a workspace `[patch]` puts in its place.
fn declaration_edges(
    manifest: &CargoToml,
    parent_dir: &Path,
    patches: &Patches,
    features: &BTreeSet<String>,
    registries: &RegistryConfig,
    child: &EdgeTarget<'_>,
    active: &[String],
) -> Vec<(String, Vec<String>)> {
    let (package, version, source) = (child.name, child.version, child.source);
    // (table, kind, target)
    type Table<'a> = (
        &'a HashMap<String, TomlDependency>,
        Option<&'a str>,
        Option<&'a str>,
    );
    let mut tables: Vec<Table<'_>> = vec![
        (&manifest.dependencies, None, None),
        (&manifest.build_dependencies, Some("build"), None),
        (&manifest.dev_dependencies, Some("dev"), None),
    ];
    for (target, spec) in &manifest.target {
        tables.extend([
            (&spec.dependencies, None, Some(target.as_str())),
            (
                &spec.build_dependencies,
                Some("build"),
                Some(target.as_str()),
            ),
            (&spec.dev_dependencies, Some("dev"), Some(target.as_str())),
        ]);
    }
    let parsed = semver::Version::parse(version).ok();
    let mut edges: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (table, kind, target) in tables {
        for (key, dep) in table {
            let (real, req) = match dep {
                TomlDependency::Simple(req) => (key.as_str(), Some(req.as_str())),
                TomlDependency::Detailed(d) => (
                    d.package_name.as_deref().unwrap_or(key),
                    d.version.as_deref(),
                ),
            };
            let from = |dep: &TomlDependency, base: &Path| match dep {
                TomlDependency::Detailed(d) if d.path.is_some() && d.git.is_none() => {
                    source.is_none()
                        && same_dir(&base.join(d.path.as_deref().unwrap_or(".")), child.dir)
                }
                _ => same_source(dep, source, registries),
            };
            // A registry declaration whose package a workspace `[patch]` replaces: the edge
            // goes to the patched package.
            let registry_declared = match dep {
                TomlDependency::Simple(_) => Some(None),
                TomlDependency::Detailed(d) if d.path.is_none() && d.git.is_none() => {
                    Some(d.registry.as_deref())
                }
                _ => None,
            };
            let from_child = from(dep, parent_dir)
                || registry_declared.is_some_and(|registry| {
                    patches
                        .replacing(registry, real, registries)
                        .is_some_and(|(patch, root)| from(patch, root))
                });
            if real != package || !from_child || !declaration_active(manifest, key, dep, features) {
                continue;
            }
            // Renamed: the manifest key is the extern name; otherwise the library's name.
            let extern_name = if real != key.as_str() {
                key.replace('-', "_")
            } else {
                child.lib_name.to_string()
            };
            let admits = match (req.map(semver::VersionReq::parse), &parsed) {
                (Some(Ok(r)), Some(v)) => r.matches(v),
                _ => true,
            };
            let kind_json = json!({"kind": kind, "target": target}).to_string();
            if admits && active.contains(&kind_json) {
                edges.entry(extern_name).or_default().insert(kind_json);
            }
        }
    }
    if edges.is_empty() {
        return vec![(child.lib_name.to_string(), active.to_vec())];
    }
    edges
        .into_iter()
        .map(|(name, kinds)| (name, kinds.into_iter().collect()))
        .collect()
}

/// The kinds of an edge, from the declarations of `child_name` that are active on the
/// plan's platform (`target`, else the host): a Windows-only build dependency is not a
/// build edge of a Linux graph.
fn dependency_kinds(
    manifest: &CargoToml,
    child_name: &str,
    target: Option<&str>,
    start_dir: &Path,
    all_platforms: bool,
) -> Vec<String> {
    let mut active = manifest.clone();
    if !all_platforms {
        active.target.retain(|spec, _| {
            !crate::adapter::rust::target_filter::is_target_cfg_incompatible_with_target(
                spec, false, target, start_dir,
            )
            .unwrap_or(false)
        });
    }
    let manifest = &active;
    let mut kinds = BTreeSet::new();
    collect_dependency_kinds(&mut kinds, &manifest.dependencies, child_name, None, None);
    collect_dependency_kinds(
        &mut kinds,
        &manifest.build_dependencies,
        child_name,
        Some("build"),
        None,
    );
    collect_dependency_kinds(
        &mut kinds,
        &manifest.dev_dependencies,
        child_name,
        Some("dev"),
        None,
    );
    for (target, spec) in &manifest.target {
        collect_dependency_kinds(
            &mut kinds,
            &spec.dependencies,
            child_name,
            None,
            Some(target),
        );
        collect_dependency_kinds(
            &mut kinds,
            &spec.build_dependencies,
            child_name,
            Some("build"),
            Some(target),
        );
        collect_dependency_kinds(
            &mut kinds,
            &spec.dev_dependencies,
            child_name,
            Some("dev"),
            Some(target),
        );
    }
    if kinds.is_empty() {
        kinds.insert(json!({"kind": null, "target": null}).to_string());
    }
    kinds.into_iter().collect()
}

fn collect_dependency_kinds(
    output: &mut BTreeSet<String>,
    dependencies: &HashMap<String, TomlDependency>,
    child_name: &str,
    kind: Option<&str>,
    target: Option<&str>,
) {
    for (alias, dependency) in dependencies {
        let real_name = match dependency {
            TomlDependency::Simple(_) => alias.as_str(),
            TomlDependency::Detailed(details) => details.package_name.as_deref().unwrap_or(alias),
        };
        if real_name == child_name {
            output.insert(json!({"kind": kind, "target": target}).to_string());
        }
    }
}

fn cargo_package_id(internal_id: &str, name: &str, version: &str, manifest: &Path) -> String {
    let source = package_source(internal_id).unwrap_or_else(|| {
        let path = manifest
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .canonicalize()
            .unwrap_or_else(|_| {
                manifest
                    .parent()
                    .unwrap_or_else(|| Path::new("."))
                    .to_path_buf()
            });
        super::project_info::path_source_url(&path)
    });
    // As Cargo writes a package ID spec: the name is left out when the URL's last path
    // segment already is the name (`path+file:///w/app#0.3.0`).
    let url_path = source.split(['?', '#']).next().unwrap_or(&source);
    if url_path.trim_end_matches('/').rsplit('/').next() == Some(name) {
        format!("{source}#{version}")
    } else {
        format!("{source}#{name}@{version}")
    }
}

fn package_source(package_id: &str) -> Option<String> {
    package_id
        .split_once(" (")
        .and_then(|(_, source)| source.strip_suffix(')'))
        .map(str::to_string)
}
