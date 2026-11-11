/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! What the last build of this command did, for its summary: how many units were
//! compiled and how many came from the cache, the wall time of each phase, and the
//! compiler that ran. The summary must never count a restored unit as compiled.

use std::sync::Mutex;
use std::time::Duration;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Report {
    pub compiled: usize,
    pub cached: usize,
    pub total: usize,
    /// Answered by the fast build lock: nothing was planned.
    pub fast_lock: bool,
    /// Wall time of the builder's phases, in order (`toolchain`, `plan`, `keys`, `compile`).
    pub phases: Vec<(String, Duration)>,
    /// First line of `rustc -vV`.
    pub rustc: Option<String>,
}

static REPORT: Mutex<Option<Report>> = Mutex::new(None);

fn update(f: impl FnOnce(&mut Report)) {
    let mut guard = REPORT.lock().unwrap_or_else(|e| e.into_inner());
    f(guard.get_or_insert_with(Report::default));
}

/// Starts a new report for a build.
pub fn begin() {
    *REPORT.lock().unwrap_or_else(|e| e.into_inner()) = Some(Report::default());
}

pub fn set_counts(compiled: usize, cached: usize, total: usize, fast_lock: bool) {
    update(|r| {
        r.compiled = compiled;
        r.cached = cached;
        r.total = total;
        r.fast_lock = fast_lock;
    });
}

pub fn set_phases(phases: Vec<(String, Duration)>) {
    update(|r| r.phases = phases);
}

pub fn set_rustc(version: &str) {
    update(|r| r.rustc = Some(version.to_string()));
}

/// The report of the last build, if one ran.
pub fn get() -> Option<Report> {
    REPORT.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// `2 compiled, 491 from the cache` and its variants, for the summary line; `verb` is
/// `compiled` or `checked`.
pub fn outcome_text(report: &Report, verb: &str) -> String {
    if report.fast_lock {
        return "Up-to-date (no-op)".to_string();
    }
    match (report.compiled, report.cached) {
        (0, 0) => "nothing to build".to_string(),
        (0, cached) => format!("{cached} from the cache, none {verb}"),
        (compiled, 0) => format!("{compiled} {verb}"),
        (compiled, cached) => format!("{compiled} {verb}, {cached} from the cache"),
    }
}

fn secs(d: Duration) -> String {
    format!("{:.2}s", d.as_secs_f64())
}

fn units(n: usize) -> String {
    if n == 1 {
        "1 unit".to_string()
    } else {
        format!("{n} units")
    }
}

/// The `time:` line: phase wall times, then the summed compiler, linker and cache times
/// (summed over units that ran in parallel, so they can exceed the wall time).
pub fn time_line(
    report: &Report,
    rustc_sum: Duration,
    link: Option<(Duration, u64)>,
    restore_sum: Duration,
) -> String {
    let mut parts: Vec<String> = report
        .phases
        .iter()
        .filter(|(name, _)| name != "toolchain")
        .map(|(name, d)| {
            let label = match name.as_str() {
                "plan" => "resolve and plan",
                "keys" => "keys",
                "compile" => "build",
                "lock_check" => "fast lock",
                other => other,
            };
            format!("{label} {}", secs(*d))
        })
        .collect();
    if report.compiled > 0 {
        let mut rustc = format!("rustc {} over {}", secs(rustc_sum), units(report.compiled));
        match link {
            Some((link, units)) if units > 0 => {
                rustc.push_str(&format!(
                    " (compile {}, link {} in {units})",
                    secs(rustc_sum.saturating_sub(link)),
                    secs(link)
                ));
            }
            _ => {}
        }
        parts.push(rustc);
    }
    if report.cached > 0 {
        parts.push(format!(
            "cache restore {} over {}",
            secs(restore_sum),
            units(report.cached)
        ));
    }
    format!("time: {}", parts.join(" · "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(compiled: usize, cached: usize) -> Report {
        Report {
            compiled,
            cached,
            total: compiled + cached,
            phases: vec![
                ("toolchain".into(), Duration::from_millis(3)),
                ("plan".into(), Duration::from_millis(400)),
                ("keys".into(), Duration::from_millis(50)),
                ("compile".into(), Duration::from_millis(11_900)),
            ],
            ..Report::default()
        }
    }

    #[test]
    fn outcome_counts_compiled_and_cached_units_apart() {
        assert_eq!(
            outcome_text(&report(2, 491), "compiled"),
            "2 compiled, 491 from the cache"
        );
        assert_eq!(outcome_text(&report(493, 0), "compiled"), "493 compiled");
        assert_eq!(
            outcome_text(&report(0, 12), "checked"),
            "12 from the cache, none checked"
        );
        let lock = Report {
            fast_lock: true,
            ..report(0, 0)
        };
        assert_eq!(outcome_text(&lock, "compiled"), "Up-to-date (no-op)");
    }

    #[test]
    fn time_line_splits_compile_and_link() {
        let line = time_line(
            &report(2, 491),
            Duration::from_millis(11_580),
            Some((Duration::from_millis(1_930), 1)),
            Duration::from_millis(730),
        );
        assert_eq!(
            line,
            "time: resolve and plan 0.40s · keys 0.05s · build 11.90s · \
             rustc 11.58s over 2 units (compile 9.65s, link 1.93s in 1) · \
             cache restore 0.73s over 491 units"
        );
        let unlinked = time_line(&report(1, 0), Duration::from_secs(1), None, Duration::ZERO);
        assert!(unlinked.ends_with("rustc 1.00s over 1 unit"), "{unlinked}");
    }
}
