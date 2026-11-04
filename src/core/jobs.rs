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

pub fn limit() -> Result<usize> {
    if let Some(raw) = std::env::var_os("MITTS_JOBS") {
        let value = raw.to_str().and_then(|value| value.parse::<usize>().ok());
        return match value {
            Some(value) if value > 0 => Ok(value),
            _ => bail!("MITTS_JOBS must be a positive integer"),
        };
    }
    Ok(std::thread::available_parallelism()
        .map(|value| value.get())
        .unwrap_or(8))
}
