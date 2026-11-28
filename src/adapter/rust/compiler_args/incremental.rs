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
    let mut identity = format!(
        "{}\n{}\n{}\nrelease={release}",
        target_dir.display(),
        session_unit(unit),
        unit.src_path.display()
    );
    // A bitcode-only build of the unit is a different output; it must not share the
    // staging directory with the object-code build (`lto_plan`).
    if unit.lto_bitcode_only {
        identity.push_str("\nlto-bitcode-only");
    }
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
    let root = inv.target_dir?.join(profile_dir).join(SESSIONS_DIR);
    prune_once(&root);
    Some(root)
}

/// The unit as its incremental session knows it: package name and manifest, target kind
/// and name, host or not. The package version is left out on purpose. A workspace has
/// one version of each member, and rustc tracks `CARGO_PKG_VERSION` like any other
/// `env!`, so a version bump reuses the session instead of opening a new one under
/// `mitts-incremental` and leaving the old one behind for good.
fn session_unit(unit: &PlannedCompilationUnit) -> String {
    format!(
        "{}\n{}\n{}:{}{}",
        unit.package_name,
        unit.manifest_path.display(),
        unit.kind,
        unit.target_name,
        if unit.for_host { ":host" } else { "" }
    )
}

/// How long a session directory may stay untouched before a build removes it.
const STALE_AFTER: std::time::Duration = std::time::Duration::from_secs(7 * 24 * 60 * 60);

static PRUNED: Mutex<std::collections::BTreeSet<PathBuf>> =
    Mutex::new(std::collections::BTreeSet::new());

/// Removes, once per process and directory, the crate session directories nobody compiled
/// into for [`STALE_AFTER`]: a feature set, a target or a crate identity no longer built
/// (sessions of earlier Mitts versions included). rustc rewrites a directory on every
/// compile into it; one only restored from the cache for a week loses its session and
/// compiles in full once on its next edit.
fn prune_once(root: &Path) {
    if PRUNED.lock().unwrap().insert(root.to_path_buf()) {
        prune_stale(root, std::time::SystemTime::now());
    }
}

fn prune_stale(root: &Path, now: std::time::SystemTime) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let stale = entry
            .metadata()
            .ok()
            .filter(|m| m.is_dir())
            .and_then(|m| m.modified().ok())
            .and_then(|t| now.duration_since(t).ok())
            .is_some_and(|age| age > STALE_AFTER);
        if stale {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
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
    let mut identity = format!(
        "mitts-incremental-v3\n{}\n{}\n{features:?}\n{effective_mode:?}\nrelease={}\n\
         {profile:?}\n{forwarded_args:?}\n{target_rustflags:?}\n{explicit_target:?}\n{toolchain}",
        session_unit(inv.unit),
        src_rel.display(),
        inv.release,
    );
    // `-C linker-plugin-lto` changes the output and is a tracked flag: the bitcode-only and
    // object-code builds of one unit need their own crate identity and session. Left out
    // when off, so existing sessions keep their key.
    if inv.unit.lto_bitcode_only && effective_mode == BuildMode::Build {
        identity.push_str("\nlto-bitcode-only");
    }
    Sha256::digest(identity.as_bytes())[..16].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_session_directories_untouched_for_a_week_are_pruned() {
        let temp = tempfile::tempdir().unwrap();
        let session = temp.path().join("vb-0jp4m9j6la1j3");
        std::fs::create_dir_all(session.join("s-1-working")).unwrap();
        std::fs::write(temp.path().join("not-a-session"), "").unwrap();
        let day = std::time::Duration::from_secs(24 * 60 * 60);

        prune_stale(temp.path(), std::time::SystemTime::now() + 6 * day);
        assert!(session.exists(), "six days old: kept");

        prune_stale(temp.path(), std::time::SystemTime::now() + 8 * day);
        assert!(!session.exists(), "eight days old: removed");
        assert!(
            temp.path().join("not-a-session").exists(),
            "files are left alone"
        );
    }

    #[test]
    fn the_session_identity_does_not_change_with_the_package_version() {
        let unit = |version: &str| PlannedCompilationUnit {
            package_id: format!("vb {version} (path+file:///ws)"),
            package_name: "vb".into(),
            version: version.into(),
            target_name: "vb".into(),
            kind: "lib".into(),
            manifest_path: "/ws/Cargo.toml".into(),
            ..Default::default()
        };
        assert_eq!(session_unit(&unit("2.9.0")), session_unit(&unit("2.9.1")));
    }
}
