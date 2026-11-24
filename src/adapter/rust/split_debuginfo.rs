/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Profile `split-debuginfo`, as Cargo applies it, and the debug-info files it produces.
//!
//! Cargo (`core/profiles.rs`, `core/compiler/mod.rs`): with debug info on, a profile that
//! sets nothing gets `unpacked` on Apple targets; the value goes to rustc as
//! `-C split-debuginfo=<v>` only when debug info is on and the target lists the value in
//! `rustc --print split-debuginfo`, and before RUSTFLAGS, so a flag there wins.
//!
//! Split debug info lives outside the rlib or executable: `.dwo` objects (Linux, unpacked),
//! a `.dwp` package (Linux, packed), object files `.o` (macOS, unpacked) or a `.dSYM`
//! bundle (macOS, packed). Whatever of them rustc left in the staging directory is stored
//! with the unit under [`PREFIX`] and placed where Cargo has it: `.dwo` / `.o` in
//! `<profile>/deps`, an executable's `.dwp` / `.dSYM` beside the executable (a library's in
//! `deps`). A cache hit restores them the same way; an entry whose files are missing from
//! the store, or name anything but such files, is not a hit.

use super::profile::UnitProfileSettings;
use super::types::PlannedCompilationUnit;
use crate::core::action_cache::CachedOutput;
use crate::core::cas::ContentAddressedStore;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

/// Cached-output prefix of split debug-info files inside an action-cache entry.
pub(crate) const PREFIX: &str = "debuginfo-out";

/// Whether the unit is built for the host (no `--target` for it), as `build_compiler_command`.
pub(crate) fn is_host_unit(unit: &PlannedCompilationUnit) -> bool {
    unit.for_host
        || matches!(
            unit.kind.as_str(),
            "proc-macro" | "custom-build" | "build-script"
        )
}

/// The `-C split-debuginfo` value the profile gives a unit built for `triple`.
pub(crate) fn effective(profile: &UnitProfileSettings, triple: &str) -> Option<String> {
    if !profile.debuginfo.is_on() {
        return None;
    }
    let value = profile
        .split_debuginfo
        .clone()
        .or_else(|| triple.contains("-apple-").then(|| "unpacked".to_string()))?;
    supported(triple).contains(&value).then_some(value)
}

/// The last `split-debuginfo` codegen option in `args`, in any spelling rustc takes
/// (`-C x`, `-Cx`, `--codegen x`, `--codegen=x`; `split-debuginfo` or `split_debuginfo`):
/// what rustc uses when the option appears more than once.
pub(crate) fn last_in(args: &[String]) -> Option<String> {
    let mut found = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let codegen = if arg == "-C" || arg == "--codegen" {
            iter.next().map(String::as_str)
        } else {
            arg.strip_prefix("--codegen=")
                .or_else(|| arg.strip_prefix("-C"))
        };
        let value = codegen.map(str::trim).and_then(|c| {
            c.strip_prefix("split-debuginfo=")
                .or_else(|| c.strip_prefix("split_debuginfo="))
        });
        if let Some(v) = value {
            found = Some(v.to_string());
        }
    }
    found
}

/// `-C split-debuginfo=unpacked` on a target whose objects name their `.dwo` files.
pub(crate) fn unpacked_dwarf(split: Option<&str>, triple: &str) -> bool {
    split == Some("unpacked") && !triple.contains("-apple-")
}

/// `<target>/<profile>/deps`, where Cargo leaves split debug info; absolute, because
/// unpacked DWARF records this path and rustc runs in another directory.
pub(crate) fn deps_dir(target_dir: &Path, release: bool) -> PathBuf {
    let deps = target_dir
        .join(super::profile::profile_dir(release))
        .join("deps");
    std::path::absolute(&deps).unwrap_or(deps)
}

static SUPPORTED: Mutex<Option<HashMap<String, Vec<String>>>> = Mutex::new(None);

/// The values `rustc --print split-debuginfo --target <triple>` lists, asked once per
/// process and target (also when rustc is a rustup proxy). A failed probe counts as "none"
/// and is remembered too: the unit key and the compiler command must see one answer.
fn supported(triple: &str) -> Vec<String> {
    let mut states = SUPPORTED.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(v) = states.as_ref().and_then(|m| m.get(triple)) {
        return v.clone();
    }
    let rustc = super::compiler_resolve::resolve_rustc();
    let host = super::compiler_resolve::resolve_host_triple();
    let target = (triple != host).then_some(triple);
    // Probed under the lock: concurrent first callers wait for one rustc run.
    let values: Vec<String> = super::toolchain_probes::rustc_print_split_debuginfo(&rustc, target)
        .map(|out| out.lines().map(|l| l.trim().to_string()).collect())
        .unwrap_or_default();
    states
        .get_or_insert_with(HashMap::new)
        .insert(triple.to_string(), values.clone());
    values
}

/// Whether `-C split-debuginfo=unpacked` could reach a non-Apple unit from any source
/// (the command line, RUSTFLAGS, the profile). The unit key then names the deps directory:
/// an over-specific key only costs sharing, a missing one could restore DWARF that names
/// another target directory.
pub(crate) fn may_be_unpacked_dwarf(triple: &str, sources: &[Option<String>]) -> bool {
    sources.iter().any(|s| unpacked_dwarf(s.as_deref(), triple))
}

/// A replacement `--remap-path-prefix` can carry: rustc splits the option at its last `=`.
pub(crate) fn remappable(path: &Path) -> bool {
    !path.to_string_lossy().contains('=')
}

/// A top-level file rustc writes beside its outputs when debug info is split, or (MSVC)
/// the program database the linker writes beside an executable or DLL.
fn is_debuginfo_file(name: &str) -> bool {
    matches!(
        Path::new(name).extension().and_then(|e| e.to_str()),
        Some("dwo" | "dwp" | "o" | "pdb")
    )
}

fn is_bundle(name: &str) -> bool {
    name.ends_with(".dSYM") && name.len() > ".dSYM".len()
}

/// A path [`staged_files`] can produce: a top-level debug-info file, or a file inside a
/// top-level `.dSYM` bundle; plain components only.
fn is_collectable(rel: &Path) -> bool {
    let parts: Vec<&str> = rel
        .components()
        .map(|c| match c {
            Component::Normal(s) => s.to_str(),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()
        .unwrap_or_default();
    match parts.as_slice() {
        [file] => is_debuginfo_file(file),
        [bundle, _, ..] => is_bundle(bundle),
        _ => false,
    }
}

/// Files of the staging directory's top level that hold split debug info, with their
/// paths relative to it (a `.dSYM` bundle contributes every file inside it).
fn staged_files(staging: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in fs::read_dir(staging).with_context(|| format!("reading {}", staging.display()))? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        let kind = entry.file_type()?;
        if kind.is_file() && is_debuginfo_file(&name) {
            out.push(PathBuf::from(&name));
        } else if kind.is_dir() && is_bundle(&name) {
            files_under(&entry.path(), Path::new(&name), &mut out)?;
        }
    }
    out.sort();
    Ok(out)
}

fn files_under(dir: &Path, rel: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let child = rel.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_dir() {
            files_under(&entry.path(), &child, out)?;
        } else if kind.is_file() {
            out.push(child);
        }
    }
    Ok(())
}

/// The staging paths of the unit's split debug-info files (for the checkout-path scan).
/// An MSVC `.pdb` is left out: the linker records the absolute paths of its objects and
/// its working directory there, which `--remap-path-prefix` does not reach, so every unit
/// would count as checkout-specific; a `.pdb` restored into another checkout only shows a
/// debugger that checkout's object paths, it changes nothing the program does.
pub(crate) fn staged_paths(staging: &Path) -> Vec<PathBuf> {
    staged_files(staging)
        .map(|files| {
            files
                .into_iter()
                .filter(|rel| rel.extension().is_none_or(|e| e != "pdb"))
                .map(|rel| staging.join(rel))
                .collect()
        })
        .unwrap_or_default()
}

/// Where a debug-info file goes. `.dwo` / `.o` objects: `<deps>/<rel>`, the directory the
/// objects name. An executable's `.dwp` or `.dSYM` bundle: beside it, as `<bin>.dwp` /
/// `<bin>.dSYM` (its rustc name has no hash, so a deps copy could clash with another
/// executable's); a library's: in `deps`.
fn destination(rel: &Path, deps_dir: &Path, dest_binary: Option<&Path>) -> PathBuf {
    let mut parts = rel.components();
    let first = parts
        .next()
        .and_then(|c| c.as_os_str().to_str())
        .unwrap_or_default();
    let rest: PathBuf = parts.collect();
    let Some(bin) = dest_binary else {
        return deps_dir.join(rel);
    };
    let bin_name = bin.file_name().unwrap_or_default().to_string_lossy();
    if is_bundle(first) {
        bin.with_file_name(format!("{bin_name}.dSYM")).join(rest)
    } else if first.ends_with(".dwp") {
        bin.with_file_name(format!("{bin_name}.dwp"))
    } else if first.ends_with(".pdb") {
        // Beside the executable under the name it records: where Windows looks for it once
        // the recorded (staging) path is gone.
        bin.with_file_name(first)
    } else {
        deps_dir.join(rel)
    }
}

/// Stores the unit's split debug-info files and places them; returns them as cached
/// outputs (empty when rustc left none).
pub(crate) fn collect(
    cas: &ContentAddressedStore,
    staging: &Path,
    deps_dir: &Path,
    dest_binary: Option<&Path>,
) -> Result<Vec<CachedOutput>> {
    let mut outputs = Vec::new();
    for rel in staged_files(staging)? {
        let (hash, size) = cas.store_file(&staging.join(&rel))?;
        place(cas, &hash, &destination(&rel, deps_dir, dest_binary))?;
        outputs.push(CachedOutput {
            relative_path: Path::new(PREFIX).join(&rel),
            cas_hash: hash,
            file_size: size,
            executable: false,
        });
    }
    Ok(outputs)
}

fn place(cas: &ContentAddressedStore, hash: &str, dest: &Path) -> Result<()> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    let _ = fs::remove_file(dest);
    cas.restore_object(hash, dest, false)
        .with_context(|| format!("placing debug info {}", dest.display()))
}

/// Splits an entry's outputs into the rest and the split debug-info files.
pub(crate) fn split(outputs: &[CachedOutput]) -> (Vec<CachedOutput>, Vec<CachedOutput>) {
    outputs
        .iter()
        .cloned()
        .partition(|o| !o.relative_path.starts_with(PREFIX))
}

/// Whether every debug-info entry names a file [`collect`] could have stored. Checked
/// before anything of a hit is restored: a damaged entry must not write elsewhere.
pub(crate) fn valid_entry(files: &[CachedOutput]) -> bool {
    files.iter().all(|o| {
        o.relative_path
            .strip_prefix(PREFIX)
            .is_ok_and(is_collectable)
    })
}

/// Restores a hit's debug-info files; `false` when one cannot be placed (then not a hit).
pub(crate) fn restore(
    cas: &ContentAddressedStore,
    files: &[CachedOutput],
    deps_dir: &Path,
    dest_binary: Option<&Path>,
) -> bool {
    valid_entry(files)
        && files.iter().all(|o| {
            let rel = o
                .relative_path
                .strip_prefix(PREFIX)
                .unwrap_or(Path::new(""));
            place(cas, &o.cas_hash, &destination(rel, deps_dir, dest_binary)).is_ok()
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn an_executables_package_and_bundle_go_beside_it_objects_in_deps() {
        let deps = Path::new("/t/debug/deps");
        let bin = Path::new("/t/debug/app");
        assert_eq!(
            destination(Path::new("app.dwp"), deps, Some(bin)),
            PathBuf::from("/t/debug/app.dwp")
        );
        assert_eq!(
            destination(
                Path::new("app.dSYM/Contents/Resources/DWARF/app"),
                deps,
                Some(bin)
            ),
            PathBuf::from("/t/debug/app.dSYM/Contents/Resources/DWARF/app")
        );
        assert_eq!(
            destination(Path::new("app.app.1a2b-cgu.0.rcgu.dwo"), deps, Some(bin)),
            deps.join("app.app.1a2b-cgu.0.rcgu.dwo")
        );
        assert_eq!(
            destination(Path::new("lib.dwp"), deps, None),
            deps.join("lib.dwp")
        );
    }

    #[test]
    fn only_paths_collect_can_produce_are_restorable() {
        assert!(is_collectable(Path::new("a-1.a.cgu.0.rcgu.dwo")));
        assert!(is_collectable(Path::new("app.dwp")));
        assert!(is_collectable(Path::new("a.o")));
        assert!(is_collectable(Path::new(
            "app.dSYM/Contents/Resources/DWARF/app"
        )));
        assert!(!is_collectable(Path::new("libfoo-1.rlib")));
        assert!(!is_collectable(Path::new("app")));
        assert!(!is_collectable(Path::new("sub/x.dwo")));
        assert!(!is_collectable(Path::new("../x.dwo")));
        assert!(!is_collectable(Path::new("/abs/x.dwo")));
        assert!(!is_collectable(Path::new(".dSYM/x")));
        assert!(!is_collectable(Path::new("")));
    }

    #[test]
    fn the_last_split_debuginfo_flag_wins() {
        assert_eq!(last_in(&args(&["-C", "opt-level=1"])), None);
        assert_eq!(
            last_in(&args(&[
                "-Csplit-debuginfo=packed",
                "-C",
                "split-debuginfo=off"
            ])),
            Some("off".into())
        );
        assert_eq!(
            last_in(&args(&["-C", "split-debuginfo=unpacked"])),
            Some("unpacked".into())
        );
        assert_eq!(
            last_in(&args(&["--codegen", "split_debuginfo=packed"])),
            Some("packed".into())
        );
        assert_eq!(
            last_in(&args(&["-Cdebuginfo=2", "--codegen=split-debuginfo=off"])),
            Some("off".into())
        );
    }
}
