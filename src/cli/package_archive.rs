/*
 * Mitts Build — source package archive
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{bail, Context, Result};
use flate2::{Compression, GzBuilder};
use std::io::{Cursor, Read};
use std::path::Path;
use std::process::Command;

fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let output = Command::new("git").current_dir(root).args(args).output()?;
    if !output.status.success() {
        bail!("git {} failed", args.join(" "));
    }
    Ok(output.stdout)
}

/// Construct a gzip tar archive from a clean, committed Git tree.
#[cfg(test)]
pub fn create(root: &Path, name: &str, version: &str) -> Result<Vec<u8>> {
    create_with_lock(root, name, version, None)
}

pub fn create_with_lock(
    root: &Path,
    name: &str,
    version: &str,
    lock: Option<&[u8]>,
) -> Result<Vec<u8>> {
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
    let top = String::from_utf8(git(root, &["rev-parse", "--show-toplevel"])?)?;
    if std::fs::canonicalize(top.trim())? != std::fs::canonicalize(root)? {
        bail!("packaging a nested workspace member is not yet supported");
    }
    if !git(root, &["status", "--porcelain", "--untracked-files=all"])?.is_empty() {
        bail!("packaging requires a clean Git checkout");
    }
    let sha = String::from_utf8(git(root, &["rev-parse", "HEAD"])?)?
        .trim()
        .to_owned();
    let tree = String::from_utf8(git(root, &["ls-tree", "-r", "HEAD"])?)?;
    if tree.lines().any(|line| line.starts_with("160000 ")) {
        bail!("packaging repositories with submodules is not yet supported");
    }
    let names = String::from_utf8(git(root, &["ls-tree", "-r", "--name-only", "-z", "HEAD"])?)?;
    let manifest_text = std::fs::read_to_string(root.join("Cargo.toml"))?;
    let manifest_data: toml::Value = toml::from_str(&manifest_text)?;
    let package_data = manifest_data
        .get("package")
        .and_then(toml::Value::as_table)
        .context("missing package table")?;
    let rules = package_data
        .get("include")
        .map(|value| (value, true))
        .or_else(|| package_data.get("exclude").map(|value| (value, false)));
    let license = package_data
        .get("license-file")
        .and_then(toml::Value::as_str)
        .map(Path::new);
    let ignored = if rules.is_some_and(|(_, include)| include) || license.is_some() {
        String::from_utf8(git(
            root,
            &[
                "ls-files",
                "--others",
                "--ignored",
                "--exclude-standard",
                "-z",
                "--",
                ".",
                ":(exclude)target/**",
            ],
        )?)?
    } else {
        String::new()
    };
    let selected = if let Some((value, include)) = rules {
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
        let paths: Vec<String> = names
            .split('\0')
            .chain(ignored.split('\0'))
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
            .collect();
        Some(super::package_patterns::select(&paths, &patterns, include)?)
    } else {
        None
    };
    let subpackages: Vec<_> = names
        .split('\0')
        .chain(ignored.split('\0'))
        .filter_map(|name| {
            let path = Path::new(name);
            (path.file_name() == Some(std::ffi::OsStr::new("Cargo.toml")))
                .then(|| path.parent())
                .flatten()
                .filter(|parent| !parent.as_os_str().is_empty())
                .map(Path::to_path_buf)
        })
        .collect();
    let bytes = git(root, &["archive", "HEAD"])?;
    let mut source = tar::Archive::new(Cursor::new(bytes));
    let gzip = GzBuilder::new()
        .mtime(0)
        .write(Vec::new(), Compression::default());
    let mut archive = tar::Builder::new(gzip);
    let prefix = format!("{name}-{version}");
    let mut manifest = None;
    let mut has_lockfile = false;
    let mut archived_paths = std::collections::HashSet::new();
    for entry in source.entries()? {
        let mut entry = entry?;
        if entry.header().entry_type().is_dir() || entry.header().entry_type().as_byte() == b'g' {
            continue;
        }
        let symlink = entry.header().entry_type().is_symlink();
        if !entry.header().entry_type().is_file() && !symlink {
            bail!("unsupported source archive entry");
        }
        let path = entry.path()?.into_owned();
        if path.starts_with("target") || subpackages.iter().any(|parent| path.starts_with(parent)) {
            continue;
        }
        if selected
            .as_ref()
            .is_some_and(|files| !files.contains(path.to_str().unwrap_or("")))
            && path != Path::new("Cargo.toml")
            && path != Path::new("Cargo.lock")
            && license != Some(path.as_path())
        {
            continue;
        }
        archived_paths.insert(path.clone());
        if path == Path::new("Cargo.lock") {
            has_lockfile = true;
        }
        if path == Path::new("Cargo.toml.orig") || path == Path::new(".cargo_vcs_info.json") {
            bail!("source contains reserved package path");
        }
        let mut contents = Vec::new();
        let mut file_mode = None;
        if symlink {
            let resolved = std::fs::canonicalize(root.join(&path))?;
            let relative = resolved
                .strip_prefix(std::fs::canonicalize(root)?)
                .context("package symlink points outside the repository")?;
            if !resolved.is_file() {
                bail!("package symlink must point to a tracked file");
            }
            let relative = relative.to_str().context("non-UTF-8 symlink target")?;
            contents = git(root, &["show", &format!("{sha}:{relative}")])?;
            if contents != std::fs::read(&resolved)? {
                bail!("symlink target differs from committed source");
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                file_mode = Some(std::fs::metadata(&resolved)?.permissions().mode() & 0o777);
            }
        } else {
            entry.read_to_end(&mut contents)?;
        }
        if path == Path::new("Cargo.lock") {
            if contents != std::fs::read(root.join("Cargo.lock"))? {
                bail!("working lockfile differs from the committed lockfile");
            }
            if let Some(expected) = lock {
                contents = expected.to_vec();
            }
        }
        if path == Path::new("Cargo.toml") {
            manifest = Some(contents.clone());
            let mut normalized: toml::Value = toml::from_str(std::str::from_utf8(&contents)?)?;
            let table = normalized
                .as_table_mut()
                .context("manifest must be a table")?;
            for section in ["workspace", "patch", "replace"] {
                table.remove(section);
            }
            contents = toml::to_string(&normalized)?.into_bytes();
        }
        // Every member gets a GNU header, like the synthesized ones below: git's ustar header must not
        // be reused, because an archive that mixes ustar and GNU headers is rejected by strict readers
        // (crates.io, issue #71). Only the permission bits are taken over from git.
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Regular);
        header.set_mode(if symlink {
            file_mode.unwrap_or(0o644)
        } else {
            entry
                .header()
                .mode()
                .map(|mode| mode & 0o777)
                .unwrap_or(0o644)
        });
        header.set_uid(0);
        header.set_gid(0);
        header.set_mtime(0);
        header.set_size(contents.len() as u64);
        header.set_cksum();
        archive.append_data(
            &mut header,
            Path::new(&prefix).join(path),
            contents.as_slice(),
        )?;
    }
    for name in names.split('\0').filter(|name| !name.is_empty()) {
        let path = Path::new(name);
        if path.starts_with("target") || subpackages.iter().any(|parent| path.starts_with(parent)) {
            continue;
        }
        let required = selected.as_ref().is_none_or(|files| files.contains(name))
            || path == Path::new("Cargo.toml")
            || path == Path::new("Cargo.lock")
            || license == Some(path);
        if required && !archived_paths.contains(path) {
            if path == Path::new("Cargo.toml.orig") || path == Path::new(".cargo_vcs_info.json") {
                bail!("source contains reserved package path");
            }
            let source_path = root.join(path);
            let source_meta = std::fs::symlink_metadata(&source_path)?;
            let resolved = if source_meta.file_type().is_symlink() {
                let resolved = std::fs::canonicalize(&source_path)?;
                resolved
                    .strip_prefix(std::fs::canonicalize(root)?)
                    .context("package symlink points outside the repository")?;
                resolved
            } else if source_meta.file_type().is_file() {
                std::fs::canonicalize(&source_path)?
            } else {
                bail!("Git omitted a non-file package entry: {name}");
            };
            if !resolved.is_file() {
                bail!("package symlink must point to a tracked file");
            }
            let relative = resolved
                .strip_prefix(std::fs::canonicalize(root)?)
                .context("package entry points outside the repository")?
                .to_str()
                .context("non-UTF-8 package path")?;
            let mut content = git(root, &["show", &format!("{sha}:{relative}")])?;
            if content != std::fs::read(&resolved)? {
                bail!("package file differs from committed source: {name}");
            }
            if path == Path::new("Cargo.toml") {
                manifest = Some(content.clone());
                let mut normalized: toml::Value = toml::from_str(std::str::from_utf8(&content)?)?;
                let table = normalized
                    .as_table_mut()
                    .context("manifest must be a table")?;
                for section in ["workspace", "patch", "replace"] {
                    table.remove(section);
                }
                content = toml::to_string(&normalized)?.into_bytes();
            }
            if path == Path::new("Cargo.lock") {
                if let Some(expected) = lock {
                    content = expected.to_vec();
                }
                has_lockfile = true;
            }
            let mut header = tar::Header::new_gnu();
            #[cfg(unix)]
            let mode = {
                use std::os::unix::fs::PermissionsExt;
                std::fs::metadata(&resolved)?.permissions().mode() & 0o777
            };
            #[cfg(not(unix))]
            let mode = 0o644;
            header.set_size(content.len() as u64);
            header.set_mode(mode);
            header.set_mtime(0);
            header.set_cksum();
            archive.append_data(
                &mut header,
                Path::new(&prefix).join(path),
                content.as_slice(),
            )?;
        }
    }
    for name in ignored.split('\0').filter(|name| !name.is_empty()) {
        let path = Path::new(name);
        if path.starts_with("target")
            || subpackages.iter().any(|parent| path.starts_with(parent))
            || path == Path::new("Cargo.lock")
            || path == Path::new("Cargo.toml")
            || path == Path::new("Cargo.toml.orig")
            || path == Path::new(".cargo_vcs_info.json")
        {
            continue;
        }
        let included = rules.is_some_and(|(_, include)| include)
            && selected.as_ref().is_some_and(|files| files.contains(name));
        if !included && license != Some(path) {
            continue;
        }
        let resolved = std::fs::canonicalize(root.join(path))?;
        if !resolved.starts_with(std::fs::canonicalize(root)?) || !resolved.is_file() {
            bail!("included ignored path must resolve to a file inside the package: {name}");
        }
        let content = std::fs::read(&resolved)?;
        let mut header = tar::Header::new_gnu();
        #[cfg(unix)]
        let mode = {
            use std::os::unix::fs::PermissionsExt;
            std::fs::metadata(&resolved)?.permissions().mode() & 0o777
        };
        #[cfg(not(unix))]
        let mode = 0o644;
        header.set_size(content.len() as u64);
        header.set_mode(mode);
        header.set_mtime(0);
        header.set_cksum();
        archive.append_data(
            &mut header,
            Path::new(&prefix).join(path),
            content.as_slice(),
        )?;
    }
    let manifest = manifest.context("missing root Cargo.toml")?;
    if !has_lockfile {
        let bytes = lock.context("missing package lockfile")?;
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_mtime(0);
        header.set_cksum();
        archive.append_data(&mut header, Path::new(&prefix).join("Cargo.lock"), bytes)?;
    }
    if std::fs::read(root.join("Cargo.toml"))? != manifest {
        bail!("working manifest differs from the committed package manifest");
    }
    let parsed: toml::Value = toml::from_str(std::str::from_utf8(&manifest)?)?;
    let package = parsed
        .get("package")
        .and_then(toml::Value::as_table)
        .context("missing package table")?;
    if package.get("name").and_then(toml::Value::as_str) != Some(name)
        || package.get("version").and_then(toml::Value::as_str) != Some(version)
    {
        bail!("package name or version differs from the committed manifest");
    }
    let vcs = serde_json::to_vec(&serde_json::json!({"git": {"sha1": sha}, "path_in_vcs": ""}))?;
    for (path, bytes) in [("Cargo.toml.orig", manifest), (".cargo_vcs_info.json", vcs)] {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_mtime(0);
        header.set_cksum();
        archive.append_data(&mut header, Path::new(&prefix).join(path), bytes.as_slice())?;
    }
    Ok(archive.into_inner()?.finish()?)
}

#[cfg(test)]
#[path = "package_archive_tests.rs"]
mod tests;
