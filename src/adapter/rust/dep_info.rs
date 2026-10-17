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
pub fn find_dep_info_files(target_dir: &Path, target_name: &str) -> Vec<PathBuf> {
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

/// Parsed outputs from a Makefile-style dep-info (.d) file.
#[derive(Debug, Default, Clone)]
pub struct ParsedDepInfo {
    pub files: Vec<PathBuf>,
    pub env_deps: Vec<(String, Option<String>)>,
}

/// Parses a Makefile-style dep-info (.d) file produced by rustc to extract included paths
/// and tracked environment variables (`# env-dep:VAR=value`), resolving relative paths against root.
pub fn parse_dep_info_full(d_path: &Path, root: Option<&Path>) -> Result<ParsedDepInfo> {
    let content = fs::read_to_string(d_path)
        .with_context(|| format!("Failed to read dep-info file {:?}", d_path))?;
    let mut parsed = ParsedDepInfo::default();

    let mut in_deps = false;

    for line in content.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("# env-dep:") {
            let rest = rest.trim();
            if let Some((var, val)) = rest.split_once('=') {
                parsed
                    .env_deps
                    .push((var.trim().to_string(), Some(val.trim().to_string())));
            } else if !rest.is_empty() {
                parsed.env_deps.push((rest.to_string(), None));
            }
            continue;
        }
        if line.starts_with('#') {
            continue;
        }

        let is_continuation = line.ends_with('\\') && !line.ends_with("\\\\");
        let (tokens_part, line_in_deps) = if let Some((_, deps_part)) = split_dep_target(line) {
            (deps_part, true)
        } else {
            (line, in_deps)
        };

        for token in tokenize_dep_info_paths(tokens_part) {
            if !token.is_empty() {
                let p = PathBuf::from(&token);
                let resolved = if p.is_relative() {
                    if let Some(r) = root {
                        r.join(&p)
                    } else {
                        p.clone()
                    }
                } else {
                    p.clone()
                };
                if line_in_deps || resolved.is_file() {
                    parsed.files.push(resolved);
                }
            }
        }
        in_deps = is_continuation && line_in_deps;
    }
    Ok(parsed)
}

fn split_dep_target(line: &str) -> Option<(&str, &str)> {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            i += 2;
            continue;
        }
        if bytes[i] == b':' {
            // Avoid splitting on Windows drive letter like C:\
            if i == 1 && bytes[0].is_ascii_alphabetic() && bytes.get(2) == Some(&b'\\') {
                i += 1;
                continue;
            }
            return Some((&line[..i], &line[i + 1..]));
        }
        i += 1;
    }
    None
}

/// Tokenizes a Makefile dependency list line, honoring `\ ` escaped spaces
/// and ignoring a trailing line-continuation `\`.
pub fn tokenize_dep_info_paths(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.peek().copied() {
                Some(' ') | Some('\t') | Some('#') | Some(':') | Some('\\') => {
                    if let Some(escaped) = chars.next() {
                        current.push(escaped);
                    }
                }
                Some(_) => {
                    current.push('\\');
                }
                None => {
                    // Trailing line-continuation backslash; ignore.
                }
            }
        } else if ch.is_whitespace() {
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
        } else {
            current.push(ch);
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// Parses a Makefile-style dep-info (.d) file produced by rustc to extract all included paths,
/// resolving relative paths against root (e.g. workspace root) if provided.
pub fn parse_dep_info_with_root(d_path: &Path, root: Option<&Path>) -> Result<Vec<PathBuf>> {
    Ok(parse_dep_info_full(d_path, root)?.files)
}

/// Parses a Makefile-style dep-info (.d) file produced by rustc to extract all included paths.
pub fn parse_dep_info(d_path: &Path) -> Result<Vec<PathBuf>> {
    parse_dep_info_with_root(d_path, None)
}
