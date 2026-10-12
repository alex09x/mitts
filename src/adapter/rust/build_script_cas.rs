/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::adapter::rust::manifest::CargoToml;
use crate::core::reflink::reflink_or_copy_dir_recursive;
use std::fs;
use std::path::Path;

use super::build_script_directives::{compute_execution_key, BuildScriptDirectives};

pub fn remap_out_dir(directives: &mut BuildScriptDirectives, from: &str, to: &str) {
    for s in &mut directives.link_searches {
        if s == from {
            *s = to.to_string();
        }
    }
    for val in directives.envs.values_mut() {
        if val.contains(from) {
            *val = val.replace(from, to);
        }
    }
}

pub fn try_restore_build_script_cas(
    cas_artifacts_dir: Option<&Path>,
    build_key: &str,
    pkg_dir: &Path,
    pkg_toml: &CargoToml,
    profile_name: &str,
    clean_crate: &str,
    out_dir: &Path,
) -> Option<BuildScriptDirectives> {
    let artifacts_dir = cas_artifacts_dir?;
    let cas_root = artifacts_dir.parent().unwrap_or(artifacts_dir);
    let script_cas_dir = cas_root.join("build_scripts").join(build_key);

    if !script_cas_dir.is_dir() {
        return None;
    }

    let entries = fs::read_dir(&script_cas_dir).ok()?;
    for entry in entries.flatten() {
        let entry_dir = entry.path();
        let file_name = entry_dir.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if file_name.starts_with('.') || !entry_dir.is_dir() {
            continue;
        }
        let meta_file = entry_dir.join("meta.toml");
        let mut cached_out_dir = entry_dir
            .join(profile_name)
            .join("build")
            .join(clean_crate)
            .join("out");
        if !cached_out_dir.is_dir() {
            cached_out_dir = entry_dir.join("out");
        }
        if !meta_file.is_file() {
            continue;
        }
        if let Ok(content) = fs::read_to_string(&meta_file) {
            if let Ok(mut directives) = toml::from_str::<BuildScriptDirectives>(&content) {
                if directives.is_fresh(pkg_dir) {
                    let mut restore_ok = true;
                    if cached_out_dir.is_dir()
                        && reflink_or_copy_dir_recursive(&cached_out_dir, out_dir).is_err()
                    {
                        restore_ok = false;
                        let _ = fs::remove_dir_all(out_dir);
                        let _ = fs::create_dir_all(out_dir);
                    }
                    if restore_ok {
                        let _ = fs::write(entry_dir.join(".last_used"), b"");
                        directives.out_dir = Some(out_dir.to_path_buf());
                        directives.links = pkg_toml.package.as_ref().and_then(|p| p.links.clone());
                        let cached_out_str = cached_out_dir.to_string_lossy();
                        let out_str = out_dir.to_string_lossy();
                        remap_out_dir(&mut directives, &cached_out_str, &out_str);
                        return Some(directives);
                    }
                }
            }
        }
    }

    None
}

pub fn save_build_script_cas(
    cas_artifacts_dir: Option<&Path>,
    build_key: &str,
    directives: &mut BuildScriptDirectives,
    profile_name: &str,
    clean_crate: &str,
    out_dir: &Path,
) {
    let artifacts_dir = match cas_artifacts_dir {
        Some(d) => d,
        None => return,
    };

    let cas_root = artifacts_dir.parent().unwrap_or(artifacts_dir);
    let script_cas_dir = cas_root.join("build_scripts").join(build_key);
    let exec_key = compute_execution_key(
        build_key,
        &directives.rerun_if_changed_hashes,
        &directives.envs,
    );
    let entry_dir = script_cas_dir.join(&exec_key);
    let cas_out = entry_dir
        .join(profile_name)
        .join("build")
        .join(clean_crate)
        .join("out");
    let out_str = out_dir.to_string_lossy();
    let cas_out_str = cas_out.to_string_lossy();

    if !entry_dir.exists() {
        let _ = fs::create_dir_all(&script_cas_dir);
        let pid = std::process::id();
        let tmp_entry_dir = script_cas_dir.join(format!(".tmp.{}.{}", exec_key, pid));
        let _ = fs::remove_dir_all(&tmp_entry_dir);

        if fs::create_dir_all(&tmp_entry_dir).is_ok() {
            let mut ok = true;
            if out_dir.is_dir() {
                let tmp_out = tmp_entry_dir
                    .join(profile_name)
                    .join("build")
                    .join(clean_crate)
                    .join("out");
                if let Some(p) = tmp_out.parent() {
                    let _ = fs::create_dir_all(p);
                }
                if reflink_or_copy_dir_recursive(out_dir, &tmp_out).is_err() {
                    ok = false;
                } else {
                    #[cfg(unix)]
                    let _ = std::os::unix::fs::symlink(&tmp_out, tmp_entry_dir.join("out"));
                }
            }
            if ok {
                let _ = fs::write(tmp_entry_dir.join(".last_used"), b"");
                let mut cas_d = directives.clone();
                cas_d.out_dir = Some(cas_out.clone());
                remap_out_dir(&mut cas_d, &out_str, &cas_out_str);
                if let Ok(toml_str) = toml::to_string(&cas_d) {
                    if fs::write(tmp_entry_dir.join("meta.toml"), toml_str).is_ok() {
                        if fs::rename(&tmp_entry_dir, &entry_dir).is_err() {
                            let _ = fs::remove_dir_all(&tmp_entry_dir);
                        }
                    } else {
                        let _ = fs::remove_dir_all(&tmp_entry_dir);
                    }
                } else {
                    let _ = fs::remove_dir_all(&tmp_entry_dir);
                }
            } else {
                let _ = fs::remove_dir_all(&tmp_entry_dir);
            }
        }
    }

    if cas_out.is_dir() {
        directives.out_dir = Some(cas_out.clone());
        remap_out_dir(directives, &out_str, &cas_out_str);
    }
}
