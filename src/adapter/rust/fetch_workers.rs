/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! How many crates `fetch_all` downloads and unpacks at once, sized to the process's
//! open-file limit. A macOS terminal starts processes with 256; Cargo fetches there, and
//! so must Mitts.

/// Descriptors left for everything else the process holds open (stdio, the store and
/// cache locks, the jobserver, the HTTP client's DNS lookups).
const RESERVED: u64 = 64;
/// What one download or one unpack worker may hold at once: a download holds its lock
/// file, a connection and a DNS socket; an unpack holds its lock file, the file it writes
/// and the directory it creates it in. One more each for the job waiting between them
/// (its lock file) and for the connection the client keeps idle after a download.
const PER_WORKER: u64 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FetchWorkers {
    pub downloads: usize,
    pub unpacks: usize,
    /// Downloaded crates that may wait for an unpack worker, each holding its lock file.
    pub queued: usize,
}

impl FetchWorkers {
    /// Workers for `missing` crates on `cpus` cores under a soft limit of `open_files`
    /// descriptors (`None`: no known limit). Without a limit: 64 downloads, an unpack per
    /// core between 4 and 32, as before.
    pub fn for_limit(missing: usize, cpus: usize, open_files: Option<u64>) -> Self {
        let mut downloads = missing.min(64);
        let mut unpacks = missing.min(cpus.clamp(4, 32));
        if let Some(limit) = open_files {
            let each = (limit.saturating_sub(RESERVED) / (2 * PER_WORKER)).max(1) as usize;
            downloads = downloads.min(each);
            unpacks = unpacks.min(each);
        }
        let downloads = downloads.max(1);
        let unpacks = unpacks.max(1);
        Self {
            downloads,
            unpacks,
            queued: unpacks,
        }
    }

    /// For this process: its soft `RLIMIT_NOFILE` on Unix.
    pub fn for_process(missing: usize) -> Self {
        let cpus = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(8);
        Self::for_limit(missing, cpus, open_file_limit())
    }
}

/// The soft open-file limit, or `None` where it is unlimited or not a thing (Windows).
pub fn open_file_limit() -> Option<u64> {
    #[cfg(unix)]
    {
        let mut lim = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim) } == 0
            && lim.rlim_cur != libc::RLIM_INFINITY
        {
            // `rlim_t` is a u64 on every Unix target Mitts ships for.
            return Some(lim.rlim_cur);
        }
        None
    }
    #[cfg(not(unix))]
    {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::FetchWorkers;

    #[test]
    fn a_high_limit_keeps_the_usual_workers() {
        let w = FetchWorkers::for_limit(500, 16, Some(1_048_576));
        assert_eq!((w.downloads, w.unpacks, w.queued), (64, 16, 16));
        assert_eq!(FetchWorkers::for_limit(500, 64, None).unpacks, 32);
    }

    #[test]
    fn a_macos_terminal_limit_bounds_what_is_open_at_once() {
        let w = FetchWorkers::for_limit(500, 32, Some(256));
        assert_eq!((w.downloads, w.unpacks, w.queued), (24, 24, 24));
        // Everything the workers may hold, plus the reserve, stays within the limit.
        let held = 3 * w.downloads + w.downloads + 3 * w.unpacks + w.queued;
        assert!(held as u64 + super::RESERVED <= 256, "{held}");
    }

    #[test]
    fn a_tiny_limit_or_few_crates_still_runs_one_of_each() {
        let w = FetchWorkers::for_limit(500, 8, Some(16));
        assert_eq!((w.downloads, w.unpacks), (1, 1));
        let w = FetchWorkers::for_limit(2, 8, Some(1 << 20));
        assert_eq!((w.downloads, w.unpacks), (2, 2));
    }
}
