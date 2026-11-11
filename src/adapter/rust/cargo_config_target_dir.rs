/*
 * Mitts Build — `build.target-dir` from Cargo configuration
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

/// `build.target-dir` from the Cargo configuration files that apply to `start_dir`, the
/// most specific file winning. A relative value is relative to the directory that holds
/// the `.cargo` directory of the file it came from, as in Cargo.
pub fn load_build_target_dir(start_dir: &Path) -> Result<Option<PathBuf>> {
    for config_path in super::cargo_config::discover_cargo_config_files(start_dir)
        .into_iter()
        .rev()
    {
        let content = fs::read_to_string(&config_path)
            .with_context(|| format!("Failed to read cargo config at {config_path:?}"))?;
        let value: toml::Value = toml::from_str(&content)
            .with_context(|| format!("Failed to parse cargo config at {config_path:?}"))?;
        let Some(dir) = value
            .get("build")
            .and_then(|build| build.get("target-dir"))
            .and_then(|dir| dir.as_str())
            .map(str::trim)
            .filter(|dir| !dir.is_empty())
        else {
            continue;
        };
        let dir = PathBuf::from(dir);
        if dir.is_absolute() {
            return Ok(Some(dir));
        }
        return Ok(Some(
            super::cargo_config_cli::base_dir(&config_path).join(dir),
        ));
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_nearest_config_wins_and_relative_paths_start_beside_its_cargo_dir() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("project");
        fs::create_dir_all(project.join(".cargo")).unwrap();
        fs::create_dir_all(root.path().join(".cargo")).unwrap();
        fs::write(
            root.path().join(".cargo/config.toml"),
            "[build]\ntarget-dir = \"outer-target\"\n",
        )
        .unwrap();
        assert_eq!(
            load_build_target_dir(&project).unwrap(),
            Some(root.path().join("outer-target"))
        );
        fs::write(
            project.join(".cargo/config.toml"),
            "[build]\ntarget-dir = \"out\"\n",
        )
        .unwrap();
        assert_eq!(
            load_build_target_dir(&project).unwrap(),
            Some(project.join("out"))
        );
    }
}
