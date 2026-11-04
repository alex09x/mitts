/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::adapter::rust::manifest::{CargoToml, DiscoveredTarget, TomlDependency};
use crate::adapter::rust::{RustAdapter, RustBuildOptions};
use crate::cli::MetadataArgs;
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
    let opt = RustBuildOptions {
        manifest_path: args.manifest_path.clone(),
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
    let plan = RustAdapter::explain_plan(&opt)?;
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

    let mut nodes = Vec::new();
    for (internal_id, entry) in &packages {
        let mut outgoing = BTreeMap::<String, BTreeSet<String>>::new();
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
                let kinds = dependency_kinds(&entry.manifest, child_name);
                for kind in kinds {
                    outgoing.entry(child_id.clone()).or_default().insert(kind);
                }
            }
        }
        let deps = outgoing
            .into_iter()
            .map(|(pkg, kinds)| {
                let name = package_names_by_id.get(&pkg).cloned().unwrap_or_default();
                let dep_kinds = kinds
                    .into_iter()
                    .map(|kind| serde_json::from_str::<Value>(&kind).unwrap_or(Value::Null))
                    .collect::<Vec<_>>();
                json!({"name": name, "pkg": pkg, "dep_kinds": dep_kinds})
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
        .map(package_json)
        .collect::<Result<Vec<_>>>()?;
    let workspace_members = packages
        .values()
        .filter(|entry| entry.is_workspace)
        .map(|entry| entry.cargo_id.clone())
        .collect::<Vec<_>>();
    let root_manifest = CargoToml::from_file(&plan.manifest_path)?;
    let root_name = args.package.clone().or_else(|| {
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

fn package_json(entry: &PackageEntry) -> Result<Value> {
    let package = entry.manifest.package.as_ref();
    let package_dir = entry
        .manifest_path
        .parent()
        .context("package manifest has no parent directory")?;
    let mut targets = entry.manifest.discover_targets(package_dir);
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
    let target_values = targets.iter().map(target_json).collect::<Vec<_>>();
    let dependencies = package_dependencies(&entry.manifest);
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
        "required-features": target.required_features,
    })
}

fn package_dependencies(manifest: &CargoToml) -> Vec<Value> {
    let mut dependencies = Vec::new();
    append_dependency_map(&mut dependencies, &manifest.dependencies, None, None);
    append_dependency_map(
        &mut dependencies,
        &manifest.build_dependencies,
        Some("build"),
        None,
    );
    append_dependency_map(
        &mut dependencies,
        &manifest.dev_dependencies,
        Some("dev"),
        None,
    );
    for (target, spec) in &manifest.target {
        append_dependency_map(&mut dependencies, &spec.dependencies, None, Some(target));
        append_dependency_map(
            &mut dependencies,
            &spec.build_dependencies,
            Some("build"),
            Some(target),
        );
        append_dependency_map(
            &mut dependencies,
            &spec.dev_dependencies,
            Some("dev"),
            Some(target),
        );
    }
    dependencies.sort_by_key(|dependency| {
        (
            dependency["name"].as_str().unwrap_or_default().to_string(),
            dependency["kind"].as_str().unwrap_or_default().to_string(),
            dependency["target"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        )
    });
    dependencies
}

fn append_dependency_map(
    output: &mut Vec<Value>,
    dependencies: &HashMap<String, TomlDependency>,
    kind: Option<&str>,
    target: Option<&str>,
) {
    for (name, dependency) in dependencies {
        let (req, rename, optional, default_features, features, source) = match dependency {
            TomlDependency::Simple(req) => (req.clone(), None, false, true, Vec::new(), None),
            TomlDependency::Detailed(details) => (
                details.version.clone().unwrap_or_else(|| "*".to_string()),
                details.package_name.clone(),
                details.optional.unwrap_or(false),
                details.default_features.unwrap_or(true),
                details.features.clone().unwrap_or_default(),
                details.git.clone(),
            ),
        };
        output.push(json!({
            "name": name,
            "source": source,
            "req": req,
            "kind": kind,
            "rename": rename,
            "optional": optional,
            "uses_default_features": default_features,
            "features": features,
            "target": target,
            "registry": null,
        }));
    }
}

fn dependency_kinds(manifest: &CargoToml, child_name: &str) -> Vec<String> {
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
        format!("path+file://{}", path.display())
    });
    format!("{source}#{name}@{version}")
}

fn package_source(package_id: &str) -> Option<String> {
    package_id
        .split_once(" (")
        .and_then(|(_, source)| source.strip_suffix(')'))
        .map(str::to_string)
}
