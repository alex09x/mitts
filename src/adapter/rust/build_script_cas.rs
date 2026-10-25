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

/// Minimum age of `.last_used` before a warm CAS hit refreshes its mtime on disk.
/// Coalesces concurrent warm-hit reads while keeping active entries fresh for GC.
pub const LAST_USED_REFRESH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);

fn touch_last_used_throttled(entry_dir: &Path) {
    let last_used = entry_dir.join(".last_used");
    let should_refresh = match fs::metadata(&last_used).and_then(|m| m.modified()) {
        Ok(mtime) => std::time::SystemTime::now()
            .duration_since(mtime)
            .map(|age| age >= LAST_USED_REFRESH_INTERVAL)
            .unwrap_or(true),
        Err(_) => true,
    };
    if should_refresh {
        let _ = fs::write(last_used, b"");
    }
}

pub fn remap_out_dir(directives: &mut BuildScriptDirectives, from: &str, to: &str) {
    if from.is_empty() || from == to {
        return;
    }
    for s in &mut directives.link_searches {
        if s.contains(from) {
            *s = s.replace(from, to);
        }
    }
    for val in directives.envs.values_mut() {
        if val.contains(from) {
            *val = val.replace(from, to);
        }
    }
    for val in directives.metadata.values_mut() {
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
    out_dir: Option<&Path>,
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
                    touch_last_used_throttled(&entry_dir);
                    let persistent_out = if cached_out_dir.is_dir() {
                        cached_out_dir
                    } else if let Some(od) = out_dir {
                        od.to_path_buf()
                    } else {
                        cached_out_dir
                    };
                    let persistent_str = persistent_out.to_string_lossy().to_string();
                    if let Some(prev) = directives
                        .out_dir
                        .as_ref()
                        .map(|p| p.to_string_lossy().to_string())
                    {
                        remap_out_dir(&mut directives, &prev, &persistent_str);
                    }
                    directives.out_dir = Some(persistent_out);
                    directives.links = pkg_toml.package.as_ref().and_then(|p| p.links.clone());
                    return Some(directives);
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
    let exec_base = format!("{}:{}", build_key, directives.freshness_digest());
    let exec_key = compute_execution_key(
        &exec_base,
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

    // An entry without its directives can never be restored: an interrupted writer or a
    // cleanup that could not delete the read-only output tree leaves one behind, and every
    // later build would re-run the script while believing the entry exists. Replace it.
    if entry_dir.exists() && !entry_dir.join("meta.toml").is_file() {
        let _ = remove_dir_all_readonly_safe(&entry_dir);
    }

    if !entry_dir.exists() {
        let _ = fs::create_dir_all(&script_cas_dir);
        let pid = std::process::id();
        let tmp_entry_dir = script_cas_dir.join(format!(".tmp.{}.{}", exec_key, pid));
        let _ = remove_dir_all_readonly_safe(&tmp_entry_dir);

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
                    make_tree_readonly_recursive(&tmp_out);
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
                            let _ = remove_dir_all_readonly_safe(&tmp_entry_dir);
                        }
                    } else {
                        let _ = remove_dir_all_readonly_safe(&tmp_entry_dir);
                    }
                } else {
                    let _ = remove_dir_all_readonly_safe(&tmp_entry_dir);
                }
            } else {
                let _ = remove_dir_all_readonly_safe(&tmp_entry_dir);
            }
        }
    }

    if cas_out.is_dir() {
        directives.out_dir = Some(cas_out.clone());
        remap_out_dir(directives, &out_str, &cas_out_str);
    }
}

/// Sets files to `0o444` and directories to `0o555` recursively (post-order on directories)
/// so shared CAS build script outputs cannot be mutated by downstream build scripts.
pub fn make_tree_readonly_recursive(dir: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                let Ok(ft) = entry.file_type() else {
                    continue;
                };
                if ft.is_symlink() {
                    continue;
                }
                if ft.is_dir() {
                    make_tree_readonly_recursive(&path);
                } else if ft.is_file() {
                    let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o444));
                }
            }
        }
        let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o555));
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
    }
}

/// Restores `0o755` on directories (and `0o644` on files) before `fs::remove_dir_all`
/// so read-only `0o555` directories inside `cas/build_scripts` can be unlinked cleanly.
pub fn remove_dir_all_readonly_safe(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        fn restore_writable(p: &Path) {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(p, fs::Permissions::from_mode(0o755));
            if let Ok(entries) = fs::read_dir(p) {
                for entry in entries.flatten() {
                    let child = entry.path();
                    let Ok(ft) = entry.file_type() else {
                        continue;
                    };
                    if ft.is_dir() {
                        restore_writable(&child);
                    } else if ft.is_file() {
                        let _ = fs::set_permissions(&child, fs::Permissions::from_mode(0o644));
                    }
                }
            }
        }
        if dir.exists() {
            restore_writable(dir);
        }
    }
    fs::remove_dir_all(dir)
}
