/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::Path;

/// Filesystem classification detected at runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilesystemKind {
    Apfs,
    Xfs,
    Btrfs,
    Ext4,
    Zfs,
    Tmpfs,
    Other(String),
}

/// Optimal zero-copy strategy supported by the underlying storage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZeroCopyStrategy {
    /// Copy-on-Write reflink (APFS clonefile on macOS, XFS/Btrfs FICLONE on Linux).
    /// Independent inode, zero physical block copying, CoW on subsequent write.
    CoWReflink,
    /// Safe read-only hardlink (ext4 on Linux where FICLONE is unsupported).
    /// Same inode, protected with read-only permissions (0o444 / 0o555), broken on atomic update.
    SafeHardlink,
    /// Full byte-by-byte physical copy fallback (e.g. cross-device link, network FS).
    FallbackCopy,
}

/// Runtime inspection report of filesystem capabilities for a given path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilesystemInfo {
    pub kind: FilesystemKind,
    pub name: String,
    pub optimal_strategy: ZeroCopyStrategy,
    pub supports_cow: bool,
    pub supports_hardlink: bool,
}

impl FilesystemInfo {
    pub fn unknown(reason: &str) -> Self {
        Self {
            kind: FilesystemKind::Other(reason.to_string()),
            name: "unknown".to_string(),
            optimal_strategy: ZeroCopyStrategy::FallbackCopy,
            supports_cow: false,
            supports_hardlink: false,
        }
    }
}

/// Probes whether copy-on-write reflink (FICLONE) works on a Linux directory by creating a temporary file.
#[cfg(target_os = "linux")]
fn probe_ficlone_support(dir: &Path) -> bool {
    use std::fs::OpenOptions;
    use std::os::unix::io::AsRawFd;

    let pid = std::process::id();
    let src_path = dir.join(format!(".mitts_ficlone_src_{}", pid));
    let dst_path = dir.join(format!(".mitts_ficlone_dst_{}", pid));

    let result = (|| {
        let src_file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&src_path)?;
        let dst_file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&dst_path)?;

        let ret = unsafe {
            crate::core::reflink::linux_sys::ioctl(
                dst_file.as_raw_fd(),
                crate::core::reflink::linux_sys::FICLONE,
                src_file.as_raw_fd(),
            )
        };
        Ok::<bool, std::io::Error>(ret == 0)
    })();

    let _ = std::fs::remove_file(&src_path);
    let _ = std::fs::remove_file(&dst_path);

    result.unwrap_or(false)
}

/// Detects the filesystem type and optimal zero-copy strategy for a path.
#[cfg(target_os = "macos")]
pub fn detect_filesystem(path: &Path) -> FilesystemInfo {
    use std::ffi::{CStr, CString};
    use std::os::unix::ffi::OsStrExt;

    let probe_path = if path.exists() {
        path.to_path_buf()
    } else {
        path.parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf()))
    };

    let c_path = match CString::new(probe_path.as_os_str().as_bytes()) {
        Ok(c) => c,
        Err(_) => return FilesystemInfo::unknown("invalid_path"),
    };

    let mut stat: libc::statfs = unsafe { std::mem::zeroed() };
    let res = unsafe { libc::statfs(c_path.as_ptr(), &mut stat) };
    if res != 0 {
        return FilesystemInfo::unknown("statfs_failed");
    }

    let fs_name = unsafe {
        CStr::from_ptr(stat.f_fstypename.as_ptr())
            .to_string_lossy()
            .to_string()
    };

    if fs_name == "apfs" {
        FilesystemInfo {
            kind: FilesystemKind::Apfs,
            name: "apfs".to_string(),
            optimal_strategy: ZeroCopyStrategy::CoWReflink,
            supports_cow: true,
            supports_hardlink: true,
        }
    } else {
        FilesystemInfo {
            kind: FilesystemKind::Other(fs_name.clone()),
            name: fs_name,
            optimal_strategy: ZeroCopyStrategy::SafeHardlink,
            supports_cow: false,
            supports_hardlink: true,
        }
    }
}

/// Detects the filesystem type and optimal zero-copy strategy for a path on Linux.
#[cfg(target_os = "linux")]
pub fn detect_filesystem(path: &Path) -> FilesystemInfo {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let probe_path = if path.exists() {
        path.to_path_buf()
    } else {
        path.parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf()))
    };

    let c_path = match CString::new(probe_path.as_os_str().as_bytes()) {
        Ok(c) => c,
        Err(_) => return FilesystemInfo::unknown("invalid_path"),
    };

    let mut stat: libc::statfs = unsafe { std::mem::zeroed() };
    let res = unsafe { libc::statfs(c_path.as_ptr(), &mut stat) };
    if res != 0 {
        return FilesystemInfo::unknown("statfs_failed");
    }

    let magic = (stat.f_type as i64) as u64;
    match magic {
        0x58465342 => FilesystemInfo {
            kind: FilesystemKind::Xfs,
            name: "xfs".to_string(),
            optimal_strategy: ZeroCopyStrategy::CoWReflink,
            supports_cow: true,
            supports_hardlink: true,
        },
        0x9123683E => FilesystemInfo {
            kind: FilesystemKind::Btrfs,
            name: "btrfs".to_string(),
            optimal_strategy: ZeroCopyStrategy::CoWReflink,
            supports_cow: true,
            supports_hardlink: true,
        },
        0xEF53 => FilesystemInfo {
            kind: FilesystemKind::Ext4,
            name: "ext4".to_string(),
            optimal_strategy: ZeroCopyStrategy::SafeHardlink,
            supports_cow: false,
            supports_hardlink: true,
        },
        0x2FC12FC1 => {
            let cow = probe_ficlone_support(&probe_path);
            FilesystemInfo {
                kind: FilesystemKind::Zfs,
                name: "zfs".to_string(),
                optimal_strategy: if cow {
                    ZeroCopyStrategy::CoWReflink
                } else {
                    ZeroCopyStrategy::SafeHardlink
                },
                supports_cow: cow,
                supports_hardlink: true,
            }
        }
        0x01021994 => FilesystemInfo {
            kind: FilesystemKind::Tmpfs,
            name: "tmpfs".to_string(),
            optimal_strategy: ZeroCopyStrategy::SafeHardlink,
            supports_cow: false,
            supports_hardlink: true,
        },
        _ => {
            let cow = probe_ficlone_support(&probe_path);
            FilesystemInfo {
                kind: FilesystemKind::Other(format!("magic:0x{:x}", magic)),
                name: if cow { "other-cow".to_string() } else { "other".to_string() },
                optimal_strategy: if cow {
                    ZeroCopyStrategy::CoWReflink
                } else {
                    ZeroCopyStrategy::SafeHardlink
                },
                supports_cow: cow,
                supports_hardlink: true,
            }
        }
    }
}

/// Fallback detection for non-unix platforms.
#[cfg(not(unix))]
pub fn detect_filesystem(_path: &Path) -> FilesystemInfo {
    FilesystemInfo {
        kind: FilesystemKind::Other("non-unix".to_string()),
        name: "non-unix".to_string(),
        optimal_strategy: ZeroCopyStrategy::FallbackCopy,
        supports_cow: false,
        supports_hardlink: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filesystem_detection_current_dir() {
        let cwd = std::env::current_dir().unwrap();
        let info = detect_filesystem(&cwd);
        assert!(!info.name.is_empty());
        #[cfg(target_os = "macos")]
        {
            assert_eq!(info.kind, FilesystemKind::Apfs);
            assert_eq!(info.optimal_strategy, ZeroCopyStrategy::CoWReflink);
            assert!(info.supports_cow);
        }
    }
}
