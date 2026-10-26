/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Which targets of a package a `CARGO_MANIFEST_DIR` read belongs to.
//!
//! Reading `CARGO_MANIFEST_DIR` keys a unit on its absolute checkout path, and every
//! unit depending on it inherits a per-checkout key. The `env!` scan is per package, so
//! one `examples/x.rs` used to lock the library and, through it, every test binary that
//! links it (actix-web: 17 units, 73 s, rebuilt in each fresh worktree). A target only
//! compiles the files under its own root, so the lock follows Cargo's layout: `src/`
//! (minus `src/bin/`) for the library and `src/main.rs`, the file plus its sibling
//! directory for `src/bin/<name>.rs`, `tests/`, `examples/` and `benches/` targets.
//! A reader outside those roots, or a target rooted elsewhere, locks everything, as
//! before; the publish-time workspace-path scan stays the safety net behind both.

use std::path::{Path, PathBuf};

const TARGET_ROOTS: [&str; 4] = ["src", "tests", "examples", "benches"];

/// True when a unit rooted at `src_path` must be keyed on the absolute manifest dir
/// because one of `readers` (files of the package that read `CARGO_MANIFEST_DIR`) can
/// be part of it.
pub fn unit_reads_manifest_dir(pkg_dir: &Path, src_path: &Path, readers: &[PathBuf]) -> bool {
    if readers.is_empty() {
        return false;
    }
    let roots: Vec<PathBuf> = TARGET_ROOTS.iter().map(|d| pkg_dir.join(d)).collect();
    if readers
        .iter()
        .any(|r| !roots.iter().any(|root| r.starts_with(root)))
    {
        return true;
    }
    let Some(scope) = TargetScope::of(pkg_dir, src_path) else {
        return true;
    };
    readers.iter().any(|r| scope.contains(r))
}

enum TargetScope {
    /// `src/` without `src/bin/`.
    Library { src: PathBuf, bin: PathBuf },
    /// One root file plus a directory of its modules.
    File { file: PathBuf, dir: PathBuf },
}

impl TargetScope {
    fn of(pkg_dir: &Path, src_path: &Path) -> Option<Self> {
        let src = pkg_dir.join("src");
        let bin = src.join("bin");
        if src_path.starts_with(&bin) {
            return Some(Self::sibling(src_path, &bin));
        }
        if src_path.starts_with(&src) {
            return Some(Self::Library { src, bin });
        }
        for root in ["tests", "examples", "benches"] {
            let root = pkg_dir.join(root);
            if src_path.starts_with(&root) {
                return Some(Self::sibling(src_path, &root));
            }
        }
        None
    }

    /// `root/name.rs` owns `root/name/`; `root/name/main.rs` owns `root/name/`.
    fn sibling(src_path: &Path, root: &Path) -> Self {
        let parent = src_path.parent().unwrap_or(root);
        let dir = if parent == root {
            root.join(src_path.file_stem().unwrap_or_default())
        } else {
            parent.to_path_buf()
        };
        Self::File {
            file: src_path.to_path_buf(),
            dir,
        }
    }

    fn contains(&self, reader: &Path) -> bool {
        match self {
            Self::Library { src, bin } => reader.starts_with(src) && !reader.starts_with(bin),
            Self::File { file, dir } => reader == file || reader.starts_with(dir),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(format!("/ws/pkg/{s}"))
    }

    #[test]
    fn an_example_reader_locks_only_the_example() {
        let readers = vec![p("examples/listing.rs")];
        let pkg = Path::new("/ws/pkg");
        assert!(!unit_reads_manifest_dir(pkg, &p("src/lib.rs"), &readers));
        assert!(!unit_reads_manifest_dir(
            pkg,
            &p("tests/traversal.rs"),
            &readers
        ));
        assert!(unit_reads_manifest_dir(
            pkg,
            &p("examples/listing.rs"),
            &readers
        ));
        assert!(!unit_reads_manifest_dir(
            pkg,
            &p("examples/other.rs"),
            &readers
        ));
    }

    #[test]
    fn a_library_reader_locks_the_library_and_its_unit_tests_only() {
        let readers = vec![p("src/util/paths.rs")];
        let pkg = Path::new("/ws/pkg");
        assert!(unit_reads_manifest_dir(pkg, &p("src/lib.rs"), &readers));
        assert!(unit_reads_manifest_dir(pkg, &p("src/main.rs"), &readers));
        assert!(!unit_reads_manifest_dir(pkg, &p("tests/t.rs"), &readers));
        assert!(!unit_reads_manifest_dir(
            pkg,
            &p("src/bin/tool.rs"),
            &readers
        ));
    }

    #[test]
    fn module_directories_belong_to_their_root_file() {
        let pkg = Path::new("/ws/pkg");
        let readers = vec![p("tests/suite/helpers.rs")];
        assert!(unit_reads_manifest_dir(
            pkg,
            &p("tests/suite/main.rs"),
            &readers
        ));
        assert!(unit_reads_manifest_dir(pkg, &p("tests/suite.rs"), &readers));
        assert!(!unit_reads_manifest_dir(
            pkg,
            &p("tests/other.rs"),
            &readers
        ));
        let bin_readers = vec![p("src/bin/tool/cli.rs")];
        assert!(unit_reads_manifest_dir(
            pkg,
            &p("src/bin/tool.rs"),
            &bin_readers
        ));
        assert!(!unit_reads_manifest_dir(
            pkg,
            &p("src/lib.rs"),
            &bin_readers
        ));
    }

    #[test]
    fn unknown_layouts_lock_everything() {
        let pkg = Path::new("/ws/pkg");
        let outside = vec![p("shared/paths.rs")];
        assert!(unit_reads_manifest_dir(pkg, &p("src/lib.rs"), &outside));
        assert!(unit_reads_manifest_dir(pkg, &p("tests/t.rs"), &outside));
        let readers = vec![p("src/lib.rs")];
        assert!(unit_reads_manifest_dir(
            pkg,
            &p("lib/custom_root.rs"),
            &readers
        ));
        assert!(!unit_reads_manifest_dir(pkg, &p("src/lib.rs"), &[]));
    }
}
