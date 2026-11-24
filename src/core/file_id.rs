/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A file's identity and hard-link count, which the store relies on: two names are the
//! same file when their device and inode agree, and a store object with more than one name
//! is still linked into a workspace. Unix reads them from the metadata; Windows from
//! `GetFileInformationByHandle` (volume serial number, file index, number of links), whose
//! std accessors are not stable yet.

use std::path::Path;

/// The identity of a file (`dev`, `ino`) and how many names it has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileId {
    pub dev: u64,
    pub ino: u64,
    pub nlink: u64,
}

impl FileId {
    /// `(dev, ino)`: equal for two names of one file.
    pub fn key(&self) -> (u64, u64) {
        (self.dev, self.ino)
    }
}

/// The file `path` names (following symlinks), or `None` when it cannot be read.
#[cfg(unix)]
pub fn of(path: &Path) -> Option<FileId> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(path).ok()?;
    Some(FileId {
        dev: meta.dev(),
        ino: meta.ino(),
        nlink: meta.nlink(),
    })
}

/// The file `path` names (following symlinks), or `None` when it cannot be read.
#[cfg(windows)]
pub fn of(path: &Path) -> Option<FileId> {
    use std::ffi::c_void;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;

    #[repr(C)]
    #[derive(Default)]
    struct ByHandleFileInformation {
        attributes: u32,
        creation: [u32; 2],
        last_access: [u32; 2],
        last_write: [u32; 2],
        volume_serial: u32,
        size_high: u32,
        size_low: u32,
        links: u32,
        index_high: u32,
        index_low: u32,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetFileInformationByHandle(file: *mut c_void, info: *mut ByHandleFileInformation)
            -> i32;
    }
    const FILE_READ_ATTRIBUTES: u32 = 0x0080;
    // Needed to open a directory; harmless for a file.
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    let file = std::fs::OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
        .ok()?;
    let mut info = ByHandleFileInformation::default();
    let ok = unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) };
    (ok != 0).then(|| FileId {
        dev: u64::from(info.volume_serial),
        ino: (u64::from(info.index_high) << 32) | u64::from(info.index_low),
        nlink: u64::from(info.links),
    })
}

/// Elsewhere no identity is known: every name is its own file.
#[cfg(not(any(unix, windows)))]
pub fn of(_path: &Path) -> Option<FileId> {
    None
}

/// Whether `a` and `b` name the same file.
pub fn same_file(a: &Path, b: &Path) -> bool {
    match (of(a), of(b)) {
        (Some(a), Some(b)) => a.key() == b.key(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hard_link_is_the_same_file_and_counts_twice() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        let c = dir.path().join("c");
        std::fs::write(&a, b"x").unwrap();
        std::fs::write(&c, b"x").unwrap();
        assert_eq!(of(&a).unwrap().nlink, 1);
        std::fs::hard_link(&a, &b).unwrap();
        assert!(same_file(&a, &b));
        assert!(!same_file(&a, &c));
        assert_eq!(of(&a).unwrap().nlink, 2);
    }
}
