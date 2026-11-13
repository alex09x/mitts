/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Which compiler and build-script processes are running right now, and how long.
//!
//! The progress line lists them, longest first. A process still running after the
//! slow-unit threshold (`--slow-units=DURATION` / `MITTS_SLOW_UNITS`, default 5m) gets a
//! line of its own with its command, and another at every further multiple of the
//! threshold; the build summary names the slowest units. Recording costs one map
//! insert and removal per process.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

pub mod watch;

pub use watch::{watch, Notice, Sink, WatchGuard};

/// The threshold when neither the flag nor the variable sets one.
pub const DEFAULT_THRESHOLD: Duration = Duration::from_secs(300);

/// `None` until configured: the default applies. `Some(None)` turns notices off.
static THRESHOLD: Mutex<Option<Option<Duration>>> = Mutex::new(None);

struct Active {
    label: String,
    started: Instant,
    command: String,
    hint: String,
    /// Multiples of the threshold already announced.
    announced: u32,
    /// The process reports the end of its front end (rustc's `.rmeta` of a pipelined
    /// library), so its time splits into front end and code generation.
    phases: bool,
    /// When the front end finished, counted from `started`.
    front_end: Option<Duration>,
}

/// The phase part of a notice: `front end so far` until the front end reports its end,
/// then `front end 1m10s, code generation 8m50s`. `None` for a process without phases.
pub fn phase_text(elapsed: Duration, phases: bool, front_end: Option<Duration>) -> Option<String> {
    match (phases, front_end) {
        (_, Some(front)) => Some(format!(
            "front end {}, code generation {}",
            format_elapsed(front),
            format_elapsed(elapsed.saturating_sub(front))
        )),
        (true, None) => Some("still in the front end (parsing, macros, type checking)".into()),
        (false, None) => None,
    }
}

static ACTIVE: Mutex<BTreeMap<u64, Active>> = Mutex::new(BTreeMap::new());
static FINISHED: Mutex<Vec<(String, Duration)>> = Mutex::new(Vec::new());
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
pub(crate) static SINK: Mutex<Option<Sink>> = Mutex::new(None);

/// Parses `30s`, `10m`, `1h`, `1h30m`, a bare number of seconds, or `off` / `0`
/// (no notices).
pub fn parse_threshold(value: &str) -> anyhow::Result<Option<Duration>> {
    let text = value.trim().to_ascii_lowercase();
    if text == "off" || text == "never" || text == "0" {
        return Ok(None);
    }
    let invalid =
        || anyhow::anyhow!("invalid duration `{value}` (expected e.g. 30s, 10m, 1h30m or off)");
    if let Ok(seconds) = text.parse::<u64>() {
        return Ok(Some(Duration::from_secs(seconds)));
    }
    let mut total = 0u64;
    let mut number = String::new();
    for ch in text.chars() {
        if ch.is_ascii_digit() {
            number.push(ch);
            continue;
        }
        let n: u64 = number.parse().map_err(|_| invalid())?;
        number.clear();
        total += match ch {
            'h' => n * 3600,
            'm' => n * 60,
            's' => n,
            _ => return Err(invalid()),
        };
    }
    if !number.is_empty() || text.is_empty() {
        return Err(invalid());
    }
    Ok((total > 0).then(|| Duration::from_secs(total)))
}

/// Sets the threshold from `--slow-units` (wins) or `MITTS_SLOW_UNITS`.
pub fn configure(flag: Option<&str>, env: Option<&str>) -> anyhow::Result<()> {
    let threshold = match (flag, env.filter(|v| !v.trim().is_empty())) {
        (Some(flag), _) => {
            parse_threshold(flag).map_err(|e| anyhow::anyhow!("--slow-units: {e}"))?
        }
        (None, Some(env)) => {
            parse_threshold(env).map_err(|e| anyhow::anyhow!("MITTS_SLOW_UNITS: {e}"))?
        }
        (None, None) => Some(DEFAULT_THRESHOLD),
    };
    *THRESHOLD.lock().unwrap_or_else(PoisonError::into_inner) = Some(threshold);
    Ok(())
}

/// The slow-unit threshold, `None` when notices are off.
pub fn threshold() -> Option<Duration> {
    THRESHOLD
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .unwrap_or(Some(DEFAULT_THRESHOLD))
}

/// Removes its process from the running set when dropped.
pub struct UnitGuard(u64);

impl Drop for UnitGuard {
    fn drop(&mut self) {
        let Some(unit) = ACTIVE
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.0)
        else {
            return;
        };
        let elapsed = unit.started.elapsed();
        if unit.announced > 0 {
            let phase = match unit.front_end {
                Some(_) => phase_text(elapsed, true, unit.front_end)
                    .map(|p| format!(" ({p})"))
                    .unwrap_or_default(),
                None => String::new(),
            };
            emit(Notice::Line(format!(
                "✓ {} finished in {}{phase}",
                unit.label,
                format_elapsed(elapsed)
            )));
        }
        FINISHED
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((unit.label, elapsed));
    }
}

/// Records a process that starts now. `command` is printed if it becomes slow; `hint`
/// (may be empty) names the settings that make it slow, such as `codegen-units=1`.
pub fn start(label: String, command: String, hint: String) -> UnitGuard {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    ACTIVE
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(
            id,
            Active {
                label,
                started: Instant::now(),
                command,
                hint,
                announced: 0,
                phases: false,
                front_end: None,
            },
        );
    UnitGuard(id)
}

impl UnitGuard {
    /// This process will report the end of its front end ([`Self::front_end_done`]).
    pub fn track_phases(&self) {
        if let Some(unit) = ACTIVE
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_mut(&self.0)
        {
            unit.phases = true;
        }
    }

    /// The front end finished now; the rest of the process is code generation.
    pub fn front_end_done(&self) {
        if let Some(unit) = ACTIVE
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_mut(&self.0)
        {
            unit.front_end.get_or_insert(unit.started.elapsed());
        }
    }
}

/// Forgets the units of the previous build.
pub fn reset() {
    FINISHED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clear();
}

/// The running processes, longest first.
pub fn running() -> Vec<(String, Duration)> {
    let mut list: Vec<(String, Duration)> = ACTIVE
        .lock()
        .unwrap()
        .values()
        .map(|unit| (unit.label.clone(), unit.started.elapsed()))
        .collect();
    list.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    list
}

/// The progress-line text for `running`: the `max` longest with their times.
pub fn running_message(running: &[(String, Duration)], max: usize) -> String {
    let mut parts: Vec<String> = running
        .iter()
        .take(max)
        .map(|(label, elapsed)| format!("{label} {}", format_elapsed(*elapsed)))
        .collect();
    if running.len() > max {
        parts.push(format!("+{} more", running.len() - max));
    }
    format!("• {}", parts.join(", "))
}

/// `42s`, `11m02s`, `1h03m`.
pub fn format_elapsed(elapsed: Duration) -> String {
    let secs = elapsed.as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m{:02}s", secs / 60, secs % 60)
    } else {
        format!("{}h{:02}m", secs / 3600, (secs % 3600) / 60)
    }
}

/// The lines due now: a process past `n` multiples of `threshold` is announced once for
/// each new multiple, with its command the first time.
pub fn due_notices(threshold: Duration) -> Vec<String> {
    let mut lines = Vec::new();
    let mut active = ACTIVE.lock().unwrap_or_else(PoisonError::into_inner);
    for unit in active.values_mut() {
        let elapsed = unit.started.elapsed();
        let multiple = (elapsed.as_nanos() / threshold.as_nanos().max(1)) as u32;
        if multiple <= unit.announced {
            continue;
        }
        let first = unit.announced == 0;
        unit.announced = multiple;
        let mut line = format!(
            "⏳ {} still running after {}",
            unit.label,
            format_elapsed(elapsed)
        );
        if let Some(phase) = phase_text(elapsed, unit.phases, unit.front_end) {
            line.push_str(&format!(": {phase}"));
        }
        if first {
            if !unit.hint.is_empty() {
                line.push_str(&format!(" ({})", unit.hint));
            }
            line.push_str(&format!("\n   {}", unit.command));
        }
        lines.push(line);
    }
    lines
}

/// The build-summary line naming the slowest units of this build: those that took at
/// least `min`, at most three.
pub fn slowest_line(min: Duration) -> Option<String> {
    let mut finished = FINISHED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    finished.retain(|(_, elapsed)| *elapsed >= min);
    if finished.is_empty() {
        return None;
    }
    finished.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let parts: Vec<String> = finished
        .iter()
        .take(3)
        .map(|(label, elapsed)| format!("{label} {}", format_elapsed(*elapsed)))
        .collect();
    Some(format!("slowest: {}", parts.join(" · ")))
}

/// Whether a printed line is one of Mitts' own notes (a slow-unit notice or the
/// `[mitts::stats]` line) rather than compiler output.
pub fn is_mitts_note(text: &str) -> bool {
    let text = text.trim_start();
    text.starts_with("⏳ ") || text.starts_with("✓ ") || text.starts_with("[mitts::")
}

pub(crate) fn emit(notice: Notice) {
    let sink: Option<Sink> = SINK.lock().unwrap_or_else(PoisonError::into_inner).clone();
    if let Some(sink) = sink {
        sink(notice);
    }
}

pub(crate) fn set_sink(sink: Option<Sink>) {
    *SINK.lock().unwrap_or_else(PoisonError::into_inner) = sink;
}

#[cfg(test)]
mod tests;
