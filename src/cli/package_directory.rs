/*
 * Mitts Build — packaging source directories without version control
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{bail, Context, Result};
use flate2::{Compression, GzBuilder};
use std::path::{Path, PathBuf};

fn discover(root: &Path, dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    if dir != root && dir.join("Cargo.toml").is_file() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path == root.join("target") || path == root.join(".git") {
            continue;
        }
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.file_type().is_dir() {
            discover(root, &path, files)?;
        } else if metadata.file_type().is_file() || metadata.file_type().is_symlink() {
            files.push(path);
        } else {
            bail!("unsupported package file type: {}", path.display());
        }
    }
    Ok(())
}

pub fn create(root: &Path, name: &str, version: &str, lock: &[u8]) -> Result<Vec<u8>> {
    if name.is_empty()
        || version.is_empty()
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        || !version
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-+".contains(&b))
    {
        bail!("invalid package name or version");
    }
    let manifest = std::fs::read(root.join("Cargo.toml"))?;
    let mut data: toml::Value = toml::from_str(std::str::from_utf8(&manifest)?)?;
    let package = data
        .get("package")
        .and_then(toml::Value::as_table)
        .context("missing package table")?;
    if package.get("name").and_then(toml::Value::as_str) != Some(name)
        || package.get("version").and_then(toml::Value::as_str) != Some(version)
    {
        bail!("package name or version differs from manifest");
    }
    let license = package
        .get("license-file")
        .and_then(toml::Value::as_str)
        .map(str::to_owned);
    let rules = package
        .get("include")
        .map(|value| (value.clone(), true))
        .or_else(|| package.get("exclude").map(|value| (value.clone(), false)));
    let mut paths = Vec::new();
    discover(root, root, &mut paths)?;
    paths.sort();
    let names: Vec<String> = paths
        .iter()
        .map(|path| {
            path.strip_prefix(root)?
                .to_str()
                .map(str::to_owned)
                .context("non-UTF-8 package path")
        })
        .collect::<Result<_>>()?;
    let selected = if let Some((value, include)) = &rules {
        let patterns: Vec<String> = value
            .as_array()
            .context("include/exclude must be an array")?
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_owned)
                    .context("package pattern must be a string")
            })
            .collect::<Result<_>>()?;
        Some(super::package_patterns::select(
            &names, &patterns, *include,
        )?)
    } else {
        None
    };
    let include_mode = rules.as_ref().is_some_and(|(_, include)| *include);
    let table = data.as_table_mut().context("manifest must be a table")?;
    for section in ["workspace", "patch", "replace"] {
        table.remove(section);
    }
    let normalized = toml::to_string(&data)?.into_bytes();
    let gzip = GzBuilder::new()
        .mtime(0)
        .write(Vec::new(), Compression::default());
    let mut archive = tar::Builder::new(gzip);
    let prefix = format!("{name}-{version}");
    for (relative, bytes) in [
        ("Cargo.toml", normalized),
        ("Cargo.toml.orig", manifest),
        ("Cargo.lock", lock.to_vec()),
    ] {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_mtime(0);
        header.set_cksum();
        archive.append_data(
            &mut header,
            Path::new(&prefix).join(relative),
            bytes.as_slice(),
        )?;
    }
    let canonical_root = std::fs::canonicalize(root)?;
    for (path, relative) in paths.into_iter().zip(names) {
        if [
            "Cargo.toml",
            "Cargo.lock",
            "Cargo.toml.orig",
            ".cargo_vcs_info.json",
        ]
        .contains(&relative.as_str())
        {
            continue;
        }
        let mandatory = license.as_deref() == Some(relative.as_str());
        let hidden = relative.split('/').any(|part| part.starts_with('.'));
        if !mandatory
            && ((!include_mode && hidden)
                || selected
                    .as_ref()
                    .is_some_and(|set| !set.contains(&relative)))
        {
            continue;
        }
        let resolved = std::fs::canonicalize(&path)?;
        if !resolved.starts_with(&canonical_root) || !resolved.is_file() {
            bail!("package path must resolve to a file inside the package: {relative}");
        }
        let bytes = std::fs::read(&resolved)?;
        let mut header = tar::Header::new_gnu();
        #[cfg(unix)]
        let mode = {
            use std::os::unix::fs::PermissionsExt;
            std::fs::metadata(&resolved)?.permissions().mode() & 0o777
        };
        #[cfg(not(unix))]
        let mode = 0o644;
        header.set_size(bytes.len() as u64);
        header.set_mode(mode);
        header.set_mtime(0);
        header.set_cksum();
        archive.append_data(
            &mut header,
            Path::new(&prefix).join(relative),
            bytes.as_slice(),
        )?;
    }
    Ok(archive.into_inner()?.finish()?)
}

#[cfg(test)]
mod tests {
    use super::create;

    #[test]
    fn include_can_select_hidden_files_and_keeps_license() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        std::fs::create_dir(root.join("src")).unwrap();
        std::fs::create_dir(root.join("nested")).unwrap();
        std::fs::write(root.join("Cargo.toml"), "[package]\nname='demo'\nversion='1.0.0'\ninclude=['/src','/.hidden']\nlicense-file='LICENSE'\n").unwrap();
        std::fs::write(root.join("src/lib.rs"), "pub fn f() {}\n").unwrap();
        std::fs::write(root.join(".hidden"), "included\n").unwrap();
        std::fs::write(root.join("LICENSE"), "license\n").unwrap();
        std::fs::write(
            root.join("nested/Cargo.toml"),
            "[package]\nname='nested'\nversion='1.0.0'\n",
        )
        .unwrap();
        let bytes = create(root, "demo", "1.0.0", b"version=4\n").unwrap();
        let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(bytes.as_slice()));
        let paths: Vec<_> = archive
            .entries()
            .unwrap()
            .map(|entry| entry.unwrap().path().unwrap().to_path_buf())
            .collect();
        assert!(paths.contains(&"demo-1.0.0/.hidden".into()));
        assert!(paths.contains(&"demo-1.0.0/LICENSE".into()));
        assert!(paths.contains(&"demo-1.0.0/src/lib.rs".into()));
        assert!(!paths.contains(&"demo-1.0.0/nested/Cargo.toml".into()));
    }
}
