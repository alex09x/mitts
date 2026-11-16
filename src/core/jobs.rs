/*
 * Mitts Build — native build job limits
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{bail, Result};
use std::sync::atomic::{AtomicUsize, Ordering};

/// `-j/--jobs N` from the command line; zero means unset. It wins over `MITTS_JOBS`.
static FLAG_LIMIT: AtomicUsize = AtomicUsize::new(0);

/// The language's own build configuration (Cargo's `build.jobs`); zero means unset. Below
/// `-j` and `MITTS_JOBS`, above the number of CPUs.
static CONFIG_LIMIT: AtomicUsize = AtomicUsize::new(0);

/// Records the limit given by `-j/--jobs`. Zero clears it.
pub fn set_flag_limit(jobs: usize) {
    FLAG_LIMIT.store(jobs, Ordering::Relaxed);
}

/// Records the limit from the build configuration. Zero clears it.
pub fn set_config_limit(jobs: usize) {
    CONFIG_LIMIT.store(jobs, Ordering::Relaxed);
}

pub fn cpus() -> usize {
    std::thread::available_parallelism()
        .map(|value| value.get())
        .unwrap_or(8)
}

pub fn limit() -> Result<usize> {
    let flag = FLAG_LIMIT.load(Ordering::Relaxed);
    if flag > 0 {
        return Ok(flag);
    }
    if let Some(raw) = std::env::var_os("MITTS_JOBS") {
        let value = raw.to_str().and_then(|value| value.parse::<usize>().ok());
        return match value {
            Some(value) if value > 0 => Ok(value),
            _ => bail!("MITTS_JOBS must be a positive integer"),
        };
    }
    let config = CONFIG_LIMIT.load(Ordering::Relaxed);
    if config > 0 {
        return Ok(config);
    }
    Ok(cpus())
}
