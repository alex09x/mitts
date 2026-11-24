/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Packages whose `OUT_DIR` is a place other builds write into. dtolnay's `scratch` bakes
//! `env!("OUT_DIR")` into its library, and `cxx-build` (any crate with a C++ bridge, such as
//! usearch) creates directories and files there from its own build script. Under Cargo that
//! is `target/<profile>/build/scratch-<hash>/out`, writable and per target directory. A
//! cached `OUT_DIR` in the store is read-only and shared, so for these packages `OUT_DIR`
//! is moved into the target directory, as Cargo has it, and the library's cache key names
//! that directory: a build never reuses a library that points into another project.

use super::build_script_cas::remap_out_dir;
use super::build_script_directives::BuildScriptDirectives;
use super::types::PlannedCompilationUnit;
use crate::core::hash::Sha256;
use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

/// Packages whose `OUT_DIR` must be writable and per target directory.
const WRITABLE_OUT_DIR_PACKAGES: [&str; 1] = ["scratch"];

pub fn needs_writable_out_dir(unit: &PlannedCompilationUnit) -> bool {
    WRITABLE_OUT_DIR_PACKAGES.contains(&unit.package_name.as_str())
}

/// `<target>/<profile>/build/<crate>-<hash>/out` for `unit`, as Cargo names it.
pub fn out_dir_in_target(
    unit: &PlannedCompilationUnit,
    target_dir: &Path,
    profile_dir: &str,
    clean_crate: &str,
) -> PathBuf {
    let hash = Sha256::digest(unit.unit_id().as_bytes());
    target_dir
        .join(profile_dir)
        .join("build")
        .join(format!("{clean_crate}-{}", &hash[..16]))
        .join("out")
}

/// Moves `directives`' `OUT_DIR` to `dest`: the files the build script produced are copied
/// there (writable) the first time; later builds keep what other build scripts added.
pub fn relocate(directives: &mut BuildScriptDirectives, dest: &Path) -> Result<()> {
    let source = directives.out_dir.clone();
    if !dest.is_dir() {
        fs::create_dir_all(dest).with_context(|| format!("create {}", dest.display()))?;
        if let Some(source) = source.as_deref().filter(|s| s.is_dir() && *s != dest) {
            copy_writable(source, dest)?;
        }
    }
    if let Some(source) = source {
        remap_out_dir(
            directives,
            &source.to_string_lossy(),
            &dest.to_string_lossy(),
        );
    }
    directives.out_dir = Some(dest.to_path_buf());
    Ok(())
}

fn copy_writable(from: &Path, to: &Path) -> Result<()> {
    for entry in fs::read_dir(from).with_context(|| format!("read {}", from.display()))? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_dir() {
            fs::create_dir_all(&target)?;
            copy_writable(&entry.path(), &target)?;
        } else if kind.is_file() {
            fs::copy(entry.path(), &target)?;
            let mut perms = fs::metadata(&target)?.permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            perms.set_readonly(false);
            fs::set_permissions(&target, perms)?;
        }
    }
    Ok(())
}

/// The cache key of a unit with a writable `OUT_DIR`: the directory it points into is part
/// of what it is, so the key carries it.
pub fn action_key(action_key: &str, compile_hash: &str, out_dir: &Path) -> String {
    let at = Sha256::digest(out_dir.to_string_lossy().as_bytes());
    format!("{action_key}:bd:{compile_hash}:writable_out:{at}")
}
