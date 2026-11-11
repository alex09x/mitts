/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Profile `incremental` for workspace units, as Cargo applies it.
//!
//! The session state lives in `<target>/<debug|release>/mitts-incremental` (Cargo's own
//! `incremental/` stays Cargo's), a per-checkout scratch directory that `mitts clean`
//! removes: it is never an input of an action key and never enters the shared cache. A
//! cache hit does not touch it; a miss (an edit) hands it to rustc, which then recompiles
//! only what the edit changed instead of the whole crate.

use crate::adapter::rust::compiler::RustcInvocation;
use crate::adapter::rust::profile::UnitProfileSettings;
use crate::adapter::rust::types::{BuildMode, PlannedCompilationUnit};
use crate::core::hash::Sha256;
use crate::core::lock::FileLock;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Directory under `<target>/<profile>` holding Mitts' rustc incremental sessions.
pub(crate) const SESSIONS_DIR: &str = "mitts-incremental";

/// Whether `unit` compiles incrementally under `profile`: workspace units only, like Cargo,
/// so registry, git and vendored dependencies stay byte-for-byte shareable.
pub(crate) fn applies(unit: &PlannedCompilationUnit, profile: &UnitProfileSettings) -> bool {
    unit.is_workspace && profile.incremental
}

/// [`applies`] with the unit's profile resolved for this build.
pub(crate) fn unit_applies(unit: &PlannedCompilationUnit, release: bool, mode: BuildMode) -> bool {
    unit.is_workspace
        && crate::adapter::rust::profile::resolve_unit_profile_for(
            &unit.manifest_path,
            &unit.package_name,
            unit.is_workspace,
            unit.for_host,
            release,
            mode == BuildMode::Test,
            unit.workspace_root.as_deref(),
        )
        .is_ok_and(|p| applies(unit, &p))
}

/// A staging directory that is the same on every build of this unit in this checkout.
///
/// rustc drops an incremental session whose tracked command-line options changed, and
/// the staging path is one of them (`--remap-path-prefix <staging>=/mitts/out/<crate>`).
/// A fresh random staging directory per build therefore made every session useless. The
/// directory is emptied before use and guarded by an exclusive lock; a concurrent build
/// of the same unit gets `None` and falls back to a private directory (correct, just not
/// incremental that time). The lock stays registered until [`release_stable_staging`]
/// (usually through a [`StableStageRelease`] guard) once the unit is done with it.
pub(crate) fn stable_staging_dir(
    cas_root: &Path,
    target_dir: &Path,
    unit: &PlannedCompilationUnit,
    clean_crate: &str,
    release: bool,
) -> Option<PathBuf> {
    let (dir, lock) = acquire_stable_staging(cas_root, target_dir, unit, clean_crate, release)?;
    HELD_STAGES.lock().unwrap().insert(dir.clone(), lock);
    Some(dir)
}

static HELD_STAGES: Mutex<BTreeMap<PathBuf, FileLock>> = Mutex::new(BTreeMap::new());

/// Releases the lock of a stable staging directory; a no-op for any other path.
pub(crate) fn release_stable_staging(dir: &Path) {
    HELD_STAGES.lock().unwrap().remove(dir);
}

/// Releases a unit's stable staging directory when the unit is done.
pub(crate) struct StableStageRelease(pub PathBuf);

impl Drop for StableStageRelease {
    fn drop(&mut self) {
        release_stable_staging(&self.0);
    }
}

fn acquire_stable_staging(
    cas_root: &Path,
    target_dir: &Path,
    unit: &PlannedCompilationUnit,
    clean_crate: &str,
    release: bool,
) -> Option<(PathBuf, FileLock)> {
    // No build mode here: `check` and `clippy` must see one path for a unit both compile
    // with rustc. Their sessions still differ by crate identity (`stable_metadata_key`).
    let identity = format!(
        "{}\n{}\n{}\nrelease={release}",
        target_dir.display(),
        unit.unit_id(),
        unit.src_path.display()
    );
    let tmp_root = cas_root.join("tmp");
    std::fs::create_dir_all(&tmp_root).ok()?;
    let dir = tmp_root.join(format!(
        "stage-incr_{clean_crate}_{}",
        &Sha256::digest(identity.as_bytes())[..16]
    ));
    let lock = FileLock::try_acquire_exclusive(&dir.with_extension("lock")).ok()??;
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir(&dir).ok()?;
    Some((dir, lock))
}

/// A stable view of the unit's build-script `OUT_DIR` for an incremental compile.
///
/// A workspace build script reruns after an edit, and each run's `OUT_DIR` lands in a new
/// cache directory. rustc sees that path in `--remap-path-prefix` (tracked) and would drop
/// the session again, so the incremental compile reads a hard-linked copy at
/// `<stable staging>/out` instead. Paths there are remapped to the same `/mitts/out/<crate>`
/// and dep-info under `<staging>/out` is normalised like `OUT_DIR`, so artifacts and keys
/// do not change. `None` (use `out_dir` as is) unless `staging` is a stable directory.
pub(crate) fn stable_out_dir(staging: &Path, out_dir: &Path) -> Option<PathBuf> {
    if !HELD_STAGES.lock().unwrap().contains_key(staging) || out_dir.starts_with(staging) {
        return None;
    }
    let view = staging.join("out");
    let _ = std::fs::remove_dir_all(&view);
    link_tree(out_dir, &view).ok()?;
    Some(view)
}

fn link_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let src = entry.path();
        let dst = to.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_dir() {
            link_tree(&src, &dst)?;
        } else if kind.is_symlink() {
            #[cfg(unix)]
            std::os::unix::fs::symlink(std::fs::read_link(&src)?, &dst)?;
        } else if std::fs::hard_link(&src, &dst).is_err() {
            std::fs::copy(&src, &dst)?;
        }
    }
    Ok(())
}

/// `-C incremental` directory for this invocation, or `None` when it does not apply.
pub(crate) fn session_root(
    inv: &RustcInvocation,
    profile: &UnitProfileSettings,
) -> Option<PathBuf> {
    if !applies(inv.unit, profile) {
        return None;
    }
    let profile_dir = &crate::adapter::rust::profile::profile_dir(inv.release);
    inv.target_dir
        .map(|t| t.join(profile_dir).join(SESSIONS_DIR))
}

/// A `-C metadata` value that stays the same across source edits of one unit, so rustc
/// finds its previous session. It covers what distinguishes two builds of the unit that
/// must not share a session or a crate identity: the unit (package, kind, target name,
/// host) and its crate root relative to the package (`tests/x.rs` and `benches/x.rs` share
/// a unit id), its features, the mode the action key uses (`effective_mode`), the
/// profile, the extra arguments rustc really gets (`forwarded_args`, filtered like the
/// action key: no `--workspace`, diagnostics markers, or a host unit's `--target` and
/// coverage flags), the target rustflags, the target and the toolchain.
pub(crate) fn stable_metadata_key(
    inv: &RustcInvocation,
    profile: &UnitProfileSettings,
    effective_mode: BuildMode,
    forwarded_args: &[String],
    target_rustflags: &[String],
    explicit_target: Option<&str>,
) -> String {
    let mut features: Vec<&str> = inv.features.iter().map(String::as_str).collect();
    features.sort_unstable();
    let pkg_dir = inv.unit.manifest_path.parent().unwrap_or(Path::new("."));
    let src_rel = inv
        .unit
        .src_path
        .strip_prefix(pkg_dir)
        .unwrap_or(&inv.unit.src_path);
    let toolchain = crate::adapter::rust::compiler_resolve::active_toolchain()
        .map(|t| t.toolchain.version.clone())
        .unwrap_or_default();
    // Host units compile for the host whatever `--target` says; their key leaves it out.
    let is_host = inv.unit.for_host
        || matches!(
            inv.unit.kind.as_str(),
            "proc-macro" | "custom-build" | "build-script"
        );
    let explicit_target = explicit_target.filter(|_| !is_host);
    let identity = format!(
        "mitts-incremental-v2\n{}\n{}\n{features:?}\n{effective_mode:?}\nrelease={}\n\
         {profile:?}\n{forwarded_args:?}\n{target_rustflags:?}\n{explicit_target:?}\n{toolchain}",
        inv.unit.unit_id(),
        src_rel.display(),
        inv.release,
    );
    Sha256::digest(identity.as_bytes())[..16].to_string()
}
