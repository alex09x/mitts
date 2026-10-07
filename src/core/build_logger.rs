/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::Result;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct PhaseRecord {
    pub name: String,
    pub duration: Duration,
    pub details: String,
}

/// Tracks granular phase timings during a build and records detailed execution traces to a log file.
pub struct BuildLogger {
    start: Instant,
    log_path: Option<PathBuf>,
    phases: Vec<PhaseRecord>,
}

pub struct PhaseTimer<'a> {
    logger: &'a mut BuildLogger,
    name: String,
    start: Instant,
}

impl<'a> PhaseTimer<'a> {
    pub fn finish_with_details(self, details: &str) {
        let dur = self.start.elapsed();
        self.logger.phases.push(PhaseRecord {
            name: self.name.clone(),
            duration: dur,
            details: details.to_string(),
        });
    }
}

impl BuildLogger {
    /// Creates a new BuildLogger configured to write traces to the specified log file path.
    pub fn new(log_path: Option<PathBuf>) -> Self {
        Self {
            start: Instant::now(),
            log_path,
            phases: Vec::new(),
        }
    }

    /// Records a completed phase with its measured duration and diagnostic detail.
    pub fn record_phase(&mut self, name: &str, duration: Duration, details: &str) {
        self.phases.push(PhaseRecord {
            name: name.to_string(),
            duration,
            details: details.to_string(),
        });
    }

    /// Measures an execution block and records its duration under the given phase name.
    pub fn measure<F, R>(&mut self, name: &str, details: &str, f: F) -> R
    where
        F: FnOnce() -> R,
    {
        let start = Instant::now();
        let res = f();
        let dur = start.elapsed();
        self.record_phase(name, dur, details);
        res
    }

    /// Starts a named phase timer that can be finished with detailed description.
    pub fn start_phase<'a>(&'a mut self, name: &str) -> PhaseTimer<'a> {
        PhaseTimer {
            logger: self,
            name: name.to_string(),
            start: Instant::now(),
        }
    }

    /// Returns recorded phase records.
    pub fn phases(&self) -> &[PhaseRecord] {
        &self.phases
    }

    /// Returns total elapsed time since logger initialization.
    pub fn total_elapsed(&self) -> Duration {
        self.start.elapsed()
    }

    /// Formats the complete timing trace and writes it to `.mitts/build.log`.
    pub fn write_trace(&self, profile: &str, exit_code: i32, outcome: &str) -> Result<()> {
        let Some(ref log_path) = self.log_path else {
            return Ok(());
        };

        if let Some(parent) = log_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let total = self.start.elapsed();
        let mut f = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(log_path)?;

        writeln!(f, "=== Mitts Build Execution Trace ===")?;
        writeln!(f, "Timestamp: {}", humantime_format())?;
        writeln!(f, "Profile:   [{}]", profile)?;
        writeln!(f, "Exit Code: {}", exit_code)?;
        writeln!(f, "Outcome:   {}", outcome)?;
        writeln!(
            f,
            "Total Dur: {:.2}s ({:.1}ms)",
            total.as_secs_f64(),
            total.as_secs_f64() * 1000.0
        )?;
        writeln!(f)?;
        writeln!(f, "Phase Breakdown:")?;
        writeln!(
            f,
            "--------------------------------------------------------------------------------"
        )?;
        writeln!(
            f,
            "{:<14} {:>10} {:>8}   DETAILS",
            "PHASE", "DURATION", "SHARE"
        )?;
        writeln!(
            f,
            "--------------------------------------------------------------------------------"
        )?;

        let total_ms = total.as_secs_f64() * 1000.0;
        for p in &self.phases {
            let p_ms = p.duration.as_secs_f64() * 1000.0;
            let share = if total_ms > 0.0 {
                (p_ms / total_ms) * 100.0
            } else {
                0.0
            };
            writeln!(
                f,
                "{:<14} {:>8.1}ms {:>7.1}%   {}",
                p.name, p_ms, share, p.details
            )?;
        }
        writeln!(
            f,
            "--------------------------------------------------------------------------------"
        )?;
        writeln!(f, "Total: {:>17.1}ms", total_ms)?;
        writeln!(
            f,
            "================================================================================"
        )?;

        Ok(())
    }
}

fn humantime_format() -> String {
    let now = std::time::SystemTime::now();
    let dt = now
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("UNIX:{}", dt)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_logger_measures_phases() {
        let temp = tempfile::tempdir().unwrap();
        let log_file = temp.path().join("build.log");
        let mut logger = BuildLogger::new(Some(log_file.clone()));

        let val = logger.measure("test_phase", "running test phase", || {
            std::thread::sleep(Duration::from_millis(5));
            42
        });
        assert_eq!(val, 42);

        logger.write_trace("dev", 0, "All targets clean").unwrap();

        let content = fs::read_to_string(&log_file).unwrap();
        assert!(content.contains("Mitts Build Execution Trace"));
        assert!(content.contains("test_phase"));
    }
}
