/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Setting a file's or directory's modification time, including on a read-only file (store
//! objects are).
//! On unix any open handle may change it. Windows needs a handle with the right to write
//! attributes, which a read-only open lacks and a write open of a read-only file is denied;
//! `FILE_WRITE_ATTRIBUTES` alone is granted for both.

use std::fs;
use std::io;
use std::path::Path;
use std::time::SystemTime;

pub fn set_modified(path: &Path, time: SystemTime) -> io::Result<()> {
    #[cfg(windows)]
    let file = {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_WRITE_ATTRIBUTES: u32 = 0x0100;
        // Needed to open a directory; harmless for a file.
        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
        fs::OpenOptions::new()
            .access_mode(FILE_WRITE_ATTRIBUTES)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(path)?
    };
    #[cfg(not(windows))]
    let file = fs::File::open(path)?;
    file.set_modified(time)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn a_read_only_file_gets_its_time() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("object");
        fs::write(&path, b"x").unwrap();
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_readonly(true);
        fs::set_permissions(&path, perms).unwrap();
        let then = SystemTime::now() - Duration::from_secs(7200);
        set_modified(&path, then).unwrap();
        let got = fs::metadata(&path).unwrap().modified().unwrap();
        let diff = got.duration_since(then).unwrap_or_else(|e| e.duration());
        assert!(diff < Duration::from_secs(2), "{diff:?}");
    }
}
