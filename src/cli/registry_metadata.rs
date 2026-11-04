/*
 * Mitts Build — crates.io package metadata
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{bail, Context, Result};
use serde_json::{json, Map, Value};
use std::path::Path;

fn dependencies(table: &toml::Table, target: Option<&str>, out: &mut Vec<Value>) -> Result<()> {
    for (section, kind) in [
        ("dependencies", "normal"),
        ("dev-dependencies", "dev"),
        ("build-dependencies", "build"),
    ] {
        let Some(deps) = table.get(section) else {
            continue;
        };
        let deps = deps
            .as_table()
            .context("dependency section must be a table")?;
        for (alias, raw) in deps {
            let mut spec = toml::Table::new();
            match raw {
                toml::Value::String(version) => {
                    spec.insert("version".into(), version.clone().into());
                }
                toml::Value::Table(value) => spec = value.clone(),
                _ => bail!("invalid dependency {alias}"),
            }
            if ["path", "git", "workspace", "registry"]
                .iter()
                .any(|key| spec.contains_key(*key))
            {
                bail!("unsupported dependency source for {alias}");
            }
            let version = spec
                .get("version")
                .and_then(toml::Value::as_str)
                .context("dependency version required")?;
            let name = spec
                .get("package")
                .and_then(toml::Value::as_str)
                .unwrap_or(alias);
            let features = spec
                .get("features")
                .cloned()
                .unwrap_or_else(|| toml::Value::Array(vec![]));
            out.push(json!({"name": name, "version_req": version, "features": features,
                "optional": spec.get("optional").and_then(toml::Value::as_bool).unwrap_or(false),
                "default_features": spec.get("default-features").and_then(toml::Value::as_bool).unwrap_or(true),
                "target": target, "kind": kind, "registry": null,
                "explicit_name_in_toml": if name == alias { None } else { Some(alias) }}));
        }
    }
    Ok(())
}

pub fn from_manifest(path: &Path) -> Result<Value> {
    let raw = std::fs::read_to_string(path)?;
    let manifest: toml::Table = toml::from_str(&raw)?;
    let package = manifest
        .get("package")
        .and_then(toml::Value::as_table)
        .context("package table required")?;
    let mut deps = Vec::new();
    dependencies(&manifest, None, &mut deps)?;
    if let Some(targets) = manifest.get("target").and_then(toml::Value::as_table) {
        for (target, table) in targets {
            dependencies(
                table.as_table().context("target must be a table")?,
                Some(target),
                &mut deps,
            )?;
        }
    }
    let mut result = Map::new();
    for (source, dest) in [
        ("name", "name"),
        ("version", "vers"),
        ("authors", "authors"),
        ("description", "description"),
        ("documentation", "documentation"),
        ("homepage", "homepage"),
        ("keywords", "keywords"),
        ("categories", "categories"),
        ("license", "license"),
        ("license-file", "license_file"),
        ("repository", "repository"),
        ("links", "links"),
        ("rust-version", "rust_version"),
    ] {
        if let Some(value) = package.get(source) {
            result.insert(dest.into(), serde_json::to_value(value)?);
        }
    }
    if !result.contains_key("name") || !result.contains_key("vers") {
        bail!("package name and version required");
    }
    result.insert("deps".into(), Value::Array(deps));
    result.insert(
        "features".into(),
        serde_json::to_value(
            manifest
                .get("features")
                .cloned()
                .unwrap_or(toml::Value::Table(Default::default())),
        )?,
    );
    let package_dir = path.parent().unwrap_or(Path::new("."));
    let readme = match package.get("readme") {
        None => ["README.md", "README.txt", "README"]
            .into_iter()
            .find(|name| package_dir.join(name).is_file()),
        Some(toml::Value::Boolean(false)) => None,
        Some(toml::Value::Boolean(true)) => Some("README.md"),
        Some(toml::Value::String(value)) => Some(value.as_str()),
        _ => bail!("unsupported readme setting"),
    };
    if let Some(readme) = readme {
        let relative = Path::new(readme);
        if relative
            .components()
            .any(|part| !matches!(part, std::path::Component::Normal(_)))
        {
            bail!("readme must be inside the package directory");
        }
        let readme_path = package_dir.join(relative);
        if readme_path.is_file() {
            if !std::fs::canonicalize(&readme_path)?
                .starts_with(std::fs::canonicalize(package_dir)?)
            {
                bail!("readme resolves outside the package directory");
            }
            result.insert(
                "readme".into(),
                std::fs::read_to_string(readme_path)?.into(),
            );
            result.insert("readme_file".into(), readme.into());
        } else if package.contains_key("readme") {
            bail!("specified readme does not exist");
        }
    }
    Ok(Value::Object(result))
}

#[cfg(test)]
mod tests {
    use super::from_manifest;

    #[test]
    fn retains_target_dependencies_and_disabled_readme() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Cargo.toml");
        std::fs::write(&path, "[package]\nname='demo'\nversion='1.0.0'\nreadme=false\n[target.'cfg(unix)'.dependencies]\nfoo={version='2',optional=true}\n").unwrap();
        std::fs::write(dir.path().join("README.md"), "not included").unwrap();
        let metadata = from_manifest(&path).unwrap();
        assert!(metadata.get("readme").is_none());
        assert_eq!(metadata["deps"][0]["target"], "cfg(unix)");
        assert_eq!(metadata["deps"][0]["optional"], true);
    }

    #[test]
    fn detects_default_readme_and_license_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Cargo.toml");
        std::fs::write(
            &path,
            "[package]\nname='demo'\nversion='1.0.0'\nlicense-file='LICENSE'\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("README.txt"), "readme text").unwrap();
        std::fs::write(dir.path().join("LICENSE"), "license text").unwrap();
        let metadata = from_manifest(&path).unwrap();
        assert_eq!(metadata["readme"], "readme text");
        assert_eq!(metadata["readme_file"], "README.txt");
        assert_eq!(metadata["license_file"], "LICENSE");
    }
}
