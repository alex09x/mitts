/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::build_script_directives::BuildScriptDirectives;
use crate::core::hash::{bytes_to_hex, Sha256};
use anyhow::Result;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

/// Assessment result of a build script's path purity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildScriptPurity {
    /// True if directives and generated OUT_DIR artifacts contain no absolute environment paths.
    pub is_pure: bool,
    /// Deterministic sorted hash of all relative files and contents in OUT_DIR.
    pub out_dir_hash: String,
    /// If impure, explanation of which forbidden path was leaked.
    pub reason: Option<String>,
}

/// Extracts the set of relative file paths inside `OUT_DIR` that are referenced in the unit's
/// `.d` dep-info file(s) inside `staging_dir`.
///
/// Note: Path-leak byte scanning covers only `OUT_DIR` files tracked in the unit's `.d` dep-info
/// (`include!`, `include_str!`, `include_bytes!`). Files in `OUT_DIR` read at runtime or by
/// untracked proc-macros are still content-hashed in `out_dir_hash`, but are not byte-scanned
/// for clone-path leaks.
pub fn extract_out_dir_files_from_dep_info(
    staging_dir: &Path,
    out_dir: &Path,
    clean_crate: &str,
    ws_root: Option<&Path>,
) -> Option<HashSet<String>> {
    let entries = fs::read_dir(staging_dir).ok()?;
    let d_files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|ext| ext == "d"))
        .collect();
    if d_files.is_empty() {
        return None;
    }

    let cleaned_out = super::unit_inputs::clean_path(out_dir);
    let cleaned_staging = super::unit_inputs::clean_path(staging_dir);
    let prefixes = [
        cleaned_out,
        cleaned_staging
            .join("debug")
            .join("build")
            .join(clean_crate)
            .join("out"),
        cleaned_staging
            .join("release")
            .join("build")
            .join(clean_crate)
            .join("out"),
        cleaned_staging.join("out"),
        PathBuf::from(format!("/mitts/out/{clean_crate}")),
    ];

    let mut referenced = HashSet::new();
    for d_path in d_files {
        let parsed = super::dep_info::parse_dep_info_full(&d_path, ws_root).ok()?;
        for file_path in parsed.files {
            let cleaned = super::unit_inputs::clean_path(&file_path);
            for prefix in &prefixes {
                if let Ok(rel) = cleaned.strip_prefix(prefix) {
                    if !rel.as_os_str().is_empty() {
                        referenced.insert(rel.to_string_lossy().to_string());
                        break;
                    }
                }
            }
        }
    }
    Some(referenced)
}

/// Scans build script directives and generated OUT_DIR artifacts for forbidden absolute paths.
pub fn assess_build_script_purity(
    pkg_dir: &Path,
    ws_root: Option<&Path>,
    staging_out_dir: &Path,
    target_dir: Option<&Path>,
    cas_root: Option<&Path>,
    directives: &BuildScriptDirectives,
) -> Result<BuildScriptPurity> {
    assess_build_script_purity_with_dep_info(
        pkg_dir,
        ws_root,
        staging_out_dir,
        target_dir,
        cas_root,
        directives,
        None,
    )
}

/// Scans build script directives and the unit's dep-info referenced OUT_DIR files for forbidden absolute paths.
pub fn assess_build_script_purity_with_dep_info(
    pkg_dir: &Path,
    ws_root: Option<&Path>,
    staging_out_dir: &Path,
    target_dir: Option<&Path>,
    cas_root: Option<&Path>,
    directives: &BuildScriptDirectives,
    referenced_out_files: Option<&HashSet<String>>,
) -> Result<BuildScriptPurity> {
    let mut forbidden_needles: Vec<Vec<u8>> = Vec::new();
    let mut add_needle = |p: &Path| {
        let s = p.to_string_lossy();
        if s.len() >= 3 {
            forbidden_needles.push(s.as_bytes().to_vec());
        }
    };

    if let Some(ws) = ws_root {
        add_needle(ws);
        if pkg_dir.starts_with(ws) {
            add_needle(pkg_dir);
        }
    } else {
        add_needle(pkg_dir);
    }
    add_needle(staging_out_dir);
    if let Some(td) = target_dir {
        add_needle(td);
    }

    let mut home_needle_opt: Option<Vec<u8>> = None;
    let mut allowed_home_prefixes: Vec<Vec<u8>> = Vec::new();

    if let Some(cas) = cas_root {
        add_needle(&cas.join("tmp"));
        let cache_root = cas.parent().unwrap_or(cas);
        let root_bytes = cache_root.to_string_lossy().as_bytes().to_vec();
        if !root_bytes.is_empty() {
            allowed_home_prefixes.push(root_bytes);
        }
        let cas_bytes = cas.to_string_lossy().as_bytes().to_vec();
        if !cas_bytes.is_empty() {
            allowed_home_prefixes.push(cas_bytes);
        }
    }

    if let Ok(home) = std::env::var("HOME") {
        if home.len() >= 3 {
            home_needle_opt = Some(home.as_bytes().to_vec());

            // Machine-wide shared paths under HOME that are valid across workspaces.
            // With an explicit CAS the cache root was already allowed above.
            if cas_root.is_none() {
                let default_root = crate::core::cache_root::default_root();
                let root_bytes = default_root.to_string_lossy().as_bytes().to_vec();
                if !root_bytes.is_empty() {
                    allowed_home_prefixes.push(root_bytes);
                }
            }

            if let Ok(cargo_home) = std::env::var("CARGO_HOME") {
                if !cargo_home.is_empty() {
                    allowed_home_prefixes.push(format!("{}/registry", cargo_home).into_bytes());
                    allowed_home_prefixes.push(format!("{}/git", cargo_home).into_bytes());
                }
            } else {
                allowed_home_prefixes.push(format!("{}/.cargo/registry", home).into_bytes());
                allowed_home_prefixes.push(format!("{}/.cargo/git", home).into_bytes());
            }

            if let Ok(rustup_home) = std::env::var("RUSTUP_HOME") {
                if !rustup_home.is_empty() {
                    allowed_home_prefixes.push(rustup_home.into_bytes());
                }
            } else {
                allowed_home_prefixes.push(format!("{}/.rustup", home).into_bytes());
            }
        }
    }

    let check_bytes = |bytes: &[u8]| -> Option<String> {
        for needle in &forbidden_needles {
            if bytes.windows(needle.len()).any(|w| w == needle.as_slice()) {
                return Some(String::from_utf8_lossy(needle).to_string());
            }
        }
        if let Some(ref home_needle) = home_needle_opt {
            let h_len = home_needle.len();
            let mut offset = 0;
            while offset + h_len <= bytes.len() {
                if &bytes[offset..offset + h_len] == home_needle.as_slice() {
                    let mut is_allowed = false;
                    for allowed in &allowed_home_prefixes {
                        if bytes[offset..].starts_with(allowed.as_slice()) {
                            is_allowed = true;
                            offset += allowed.len();
                            break;
                        }
                    }
                    if !is_allowed {
                        return Some(String::from_utf8_lossy(home_needle).to_string());
                    }
                } else {
                    offset += 1;
                }
            }
        }
        None
    };

    // 1. Scan directives
    for (label, list) in [
        ("rustc-cfg", &directives.cfgs),
        ("rustc-flag", &directives.flags),
        ("rustc-link-lib", &directives.link_libs),
        ("rustc-link-arg", &directives.link_args),
    ] {
        for item in list {
            if let Some(leak) = check_bytes(item.as_bytes()) {
                return Ok(BuildScriptPurity {
                    is_pure: false,
                    out_dir_hash: String::new(),
                    reason: Some(format!("{label} contains absolute path needle '{leak}'")),
                });
            }
        }
    }
    for s in &directives.link_searches {
        let path_str = if let Some((_kind, path)) = s.split_once('=') {
            path
        } else {
            s.as_str()
        };
        let p = Path::new(path_str);
        let is_out_dir = p == staging_out_dir
            || p.starts_with(staging_out_dir)
            || directives
                .out_dir
                .as_deref()
                .is_some_and(|d| p == d || p.starts_with(d))
            || cas_root.is_some_and(|cas| p.starts_with(cas.join("build_scripts")))
            || (p.ends_with("out") && p.to_string_lossy().contains("build_scripts"));
        if is_out_dir {
            continue;
        }
        if let Some(leak) = check_bytes(s.as_bytes()) {
            return Ok(BuildScriptPurity {
                is_pure: false,
                out_dir_hash: String::new(),
                reason: Some(format!(
                    "rustc-link-search contains absolute path needle '{}'",
                    leak
                )),
            });
        }
    }
    for (k, v) in &directives.envs {
        let v_scrubbed = {
            let mut s = v.clone();
            let staging_str = staging_out_dir.to_string_lossy();
            if s.contains(staging_str.as_ref()) {
                s = s.replace(staging_str.as_ref(), "<OUT_DIR>");
            }
            if let Some(ref d) = directives.out_dir {
                let d_str = d.to_string_lossy();
                if s.contains(d_str.as_ref()) {
                    s = s.replace(d_str.as_ref(), "<OUT_DIR>");
                }
            }
            s
        };
        if let Some(leak) = check_bytes(k.as_bytes()).or_else(|| check_bytes(v_scrubbed.as_bytes()))
        {
            return Ok(BuildScriptPurity {
                is_pure: false,
                out_dir_hash: String::new(),
                reason: Some(format!(
                    "rustc-env contains absolute path needle '{}'",
                    leak
                )),
            });
        }
    }

    // 2. Scan OUT_DIR files recursively and compute out_dir_hash
    let mut out_dir_entries: Vec<(String, String, String)> = Vec::new();
    let mut leak_detected: Option<String> = None;

    if staging_out_dir.is_dir() {
        let mut stack: Vec<PathBuf> = vec![staging_out_dir.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let read_res = fs::read_dir(&dir);
            if let Ok(entries) = read_res {
                let mut dir_empty = true;
                for entry in entries.flatten() {
                    dir_empty = false;
                    let path = entry.path();
                    let file_type = entry.file_type();
                    let is_symlink = file_type.as_ref().map(|t| t.is_symlink()).unwrap_or(false);

                    let rel = path
                        .strip_prefix(staging_out_dir)
                        .unwrap_or(&path)
                        .to_string_lossy()
                        .to_string();
                    let should_check_leak = match referenced_out_files {
                        Some(set) => set.contains(&rel),
                        None => true,
                    };

                    if is_symlink {
                        let target = fs::read_link(&path).unwrap_or_default();
                        let target_str = target.to_string_lossy().to_string();
                        if should_check_leak {
                            if let Some(leak) = check_bytes(target_str.as_bytes()) {
                                leak_detected = Some(format!(
                                    "Symlink target '{}' at '{}' contains absolute path needle '{}'",
                                    target_str, rel, leak
                                ));
                            } else if path.is_file() {
                                let target_bytes = fs::read(&path).unwrap_or_default();
                                if let Some(leak) = check_bytes(&target_bytes) {
                                    leak_detected = Some(format!(
                                        "File '{}' in OUT_DIR contains absolute path needle '{}'",
                                        rel, leak
                                    ));
                                }
                            }
                        }
                        let target_hash = Sha256::digest(target_str.as_bytes());
                        out_dir_entries.push((rel, "symlink".to_string(), target_hash));
                    } else if path.is_dir() {
                        stack.push(path);
                    } else if path.is_file() {
                        if rel.ends_with(".d") && !should_check_leak {
                            continue;
                        }
                        let content_bytes = fs::read(&path).unwrap_or_default();
                        if should_check_leak {
                            if let Some(leak) = check_bytes(&content_bytes) {
                                leak_detected = Some(format!(
                                    "File '{}' in OUT_DIR contains absolute path needle '{}'",
                                    rel, leak
                                ));
                            }
                        }
                        let content_hash = Sha256::digest(&content_bytes);
                        out_dir_entries.push((rel, "file".to_string(), content_hash));
                    }
                }
                if dir_empty && dir != *staging_out_dir {
                    let rel = dir
                        .strip_prefix(staging_out_dir)
                        .unwrap_or(&dir)
                        .to_string_lossy()
                        .to_string();
                    out_dir_entries.push((rel, "dir".to_string(), String::new()));
                }
            }
        }
    }

    out_dir_entries.sort_by(|a, b| a.0.cmp(&b.0));

    let mut out_hasher = Sha256::new();
    out_hasher.update(b"out_dir_entries_v1:\n");
    for (rel, kind, h) in &out_dir_entries {
        for chunk in [
            b"path:" as &[u8],
            rel.as_bytes(),
            b":kind:",
            kind.as_bytes(),
            b":hash:",
            h.as_bytes(),
            b"\n",
        ] {
            out_hasher.update(chunk);
        }
    }
    let out_dir_hash = bytes_to_hex(&out_hasher.finalize());

    Ok(BuildScriptPurity {
        is_pure: leak_detected.is_none(),
        out_dir_hash,
        reason: leak_detected,
    })
}
