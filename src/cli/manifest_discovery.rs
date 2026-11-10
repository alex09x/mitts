/*
 * Mitts Build — locate the project manifest the way Cargo does
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::adapter::rust::manifest::CargoToml;
use crate::adapter::RustBuildOptions;
use std::path::{Path, PathBuf};

/// The manifest a command works on: `--manifest-path` when given, otherwise, like Cargo,
/// the nearest `Cargo.toml` in the current directory or one of its parents. The result is
/// absolute, so later lookups that climb parent directories see the real filesystem.
///
/// `None` (no manifest anywhere up the tree) keeps the default `./Cargo.toml` error.
pub fn discover_manifest(explicit: &Option<PathBuf>) -> Option<PathBuf> {
    if let Some(path) = explicit {
        return Some(std::path::absolute(path).unwrap_or_else(|_| path.clone()));
    }
    nearest_manifest(&std::env::current_dir().ok()?)
}

fn nearest_manifest(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .map(|dir| dir.join("Cargo.toml"))
        .find(|candidate| candidate.is_file())
}

/// Like Cargo, a package manifest that belongs to an enclosing workspace is built as that
/// workspace: the workspace root's manifest, `Cargo.lock`, members, `[patch]` and `target/`,
/// with the member selected unless packages (or the whole workspace) were named.
pub fn select_workspace_member(opt: &mut RustBuildOptions, whole_workspace: bool) {
    let Some(manifest) = opt.manifest_path.clone() else {
        return;
    };
    let Some((workspace_manifest, member)) = enclosing_workspace(&manifest) else {
        return;
    };
    opt.manifest_path = Some(workspace_manifest);
    if !whole_workspace && opt.package.is_none() && opt.packages.is_empty() {
        opt.package = Some(member);
    }
}

/// The enclosing workspace's root manifest and the member's package name, when `manifest`
/// is a package (not itself the workspace root) listed in an outer workspace.
fn enclosing_workspace(manifest: &Path) -> Option<(PathBuf, String)> {
    let toml = CargoToml::from_file(manifest).ok()?;
    if toml.workspace.is_some() {
        return None;
    }
    let name = toml.package.as_ref()?.name.clone();
    let (root, _) = CargoToml::find_workspace_root(manifest, &toml).ok()??;
    (root != manifest).then_some((root, name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn climbs_to_the_nearest_parent_manifest() {
        let temp = tempfile::TempDir::new().unwrap();
        let member = temp.path().join("member");
        std::fs::create_dir_all(member.join("src/nested")).unwrap();
        std::fs::write(
            temp.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"member\"]\n",
        )
        .unwrap();
        std::fs::write(
            member.join("Cargo.toml"),
            "[package]\nname = \"member\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();

        assert_eq!(
            nearest_manifest(&member.join("src/nested")),
            Some(member.join("Cargo.toml"))
        );
        assert_eq!(nearest_manifest(&member), Some(member.join("Cargo.toml")));
        assert!(discover_manifest(&Some(PathBuf::from("x/Cargo.toml")))
            .unwrap()
            .is_absolute());

        let mut opt = RustBuildOptions {
            manifest_path: Some(member.join("Cargo.toml")),
            ..Default::default()
        };
        select_workspace_member(&mut opt, false);
        assert_eq!(opt.manifest_path, Some(temp.path().join("Cargo.toml")));
        assert_eq!(opt.package.as_deref(), Some("member"));

        let mut whole = RustBuildOptions {
            manifest_path: Some(member.join("Cargo.toml")),
            ..Default::default()
        };
        select_workspace_member(&mut whole, true);
        assert_eq!(whole.manifest_path, Some(temp.path().join("Cargo.toml")));
        assert_eq!(whole.package, None);

        let mut root = RustBuildOptions {
            manifest_path: Some(temp.path().join("Cargo.toml")),
            ..Default::default()
        };
        select_workspace_member(&mut root, false);
        assert_eq!(root.package, None);
    }
}
