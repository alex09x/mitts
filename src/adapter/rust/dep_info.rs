/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
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

/// Discovers all matching dep-info (.d) files produced by rustc/cargo for the target.
pub(crate) fn find_dep_info_files(target_dir: &Path, target_name: &str) -> Vec<PathBuf> {
    let mut d_files = Vec::new();
    let norm_target = target_name.replace('-', "_");
    for profile in &["debug", "release"] {
        let p_dir = target_dir.join(profile);
        if !p_dir.is_dir() {
            continue;
        }
        let direct = p_dir.join(format!("{}.d", target_name));
        if direct.is_file() {
            d_files.push(direct);
        }
        let direct_norm = p_dir.join(format!("{}.d", norm_target));
        if direct_norm.is_file() && direct_norm != p_dir.join(format!("{}.d", target_name)) {
            d_files.push(direct_norm);
        }
        let deps_dir = p_dir.join("deps");
        let direct_dep = deps_dir.join(format!("{}.d", target_name));
        if direct_dep.is_file() {
            d_files.push(direct_dep);
        }
        let direct_dep_norm = deps_dir.join(format!("{}.d", norm_target));
        if direct_dep_norm.is_file()
            && direct_dep_norm != deps_dir.join(format!("{}.d", target_name))
        {
            d_files.push(direct_dep_norm);
        }
    }
    d_files
}

/// Parses a Makefile-style dep-info (.d) file produced by rustc to extract all included paths.
pub(crate) fn parse_dep_info(d_path: &Path) -> Result<Vec<PathBuf>> {
    let content = fs::read_to_string(d_path)
        .with_context(|| format!("Failed to read dep-info file {:?}", d_path))?;
    let mut files = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        let tokens_part = if let Some((_, deps_part)) = line.split_once(':') {
            deps_part
        } else {
            line
        };
        for token in tokens_part.split_whitespace() {
            let token = token.trim_end_matches('\\');
            if !token.is_empty() {
                let p = PathBuf::from(token);
                if p.is_file() {
                    files.push(p);
                }
            }
        }
    }
    Ok(files)
}
