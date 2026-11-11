/*
 * Mitts Build — per-unit timing report (`--timings`)
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! `--timings` records when every unit of the build started and how long it took, and
//! writes a self-contained HTML report where Cargo puts its own:
//! `target/cargo-timings/cargo-timing-<unix-seconds>.html`, copied to `cargo-timing.html`.
//! `--timings=json` writes the same records as JSON next to it. Recording is off unless a
//! command enabled it; then each unit costs one lock and two clock reads.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Html,
    Json,
}

#[derive(Debug, Clone)]
pub struct UnitTiming {
    pub unit: String,
    pub start_secs: f64,
    pub duration_secs: f64,
    /// `compiled`, `restored` (taken from the cache) or `failed`.
    pub outcome: &'static str,
    /// Time in the linker, when it was measured (see `link_timing`).
    pub link_secs: Option<f64>,
}

#[derive(Default, Clone, Copy)]
struct UnitNote {
    restored: bool,
    link: Option<std::time::Duration>,
}

thread_local! {
    // A unit runs on one worker thread from `unit_started` to `unit_finished`.
    static UNIT_NOTE: std::cell::Cell<UnitNote> = const {
        std::cell::Cell::new(UnitNote { restored: false, link: None })
    };
}

/// Notes that the unit running on this thread was restored from the cache.
pub fn note_restored() {
    UNIT_NOTE.with(|n| {
        n.set(UnitNote {
            restored: true,
            ..n.get()
        })
    });
}

/// Notes the link time of the unit running on this thread.
pub fn note_link(duration: std::time::Duration) {
    UNIT_NOTE.with(|n| {
        n.set(UnitNote {
            link: Some(duration),
            ..n.get()
        })
    });
}

struct Recorder {
    started: Instant,
    formats: Vec<Format>,
    units: Vec<UnitTiming>,
    target_dir: Option<PathBuf>,
    title: String,
}

static RECORDER: Mutex<Option<Recorder>> = Mutex::new(None);

/// Parses `--timings` / `--timings=html,json` (no value means html).
pub fn parse_formats(value: &str) -> Result<Vec<Format>> {
    let mut formats = Vec::new();
    for part in value.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let format = match part {
            "html" => Format::Html,
            "json" => Format::Json,
            other => anyhow::bail!("--timings: unknown output `{other}` (expected html or json)"),
        };
        if !formats.contains(&format) {
            formats.push(format);
        }
    }
    if formats.is_empty() {
        formats.push(Format::Html);
    }
    Ok(formats)
}

/// Starts recording for this command.
pub fn enable(formats: Vec<Format>) {
    *RECORDER.lock().unwrap_or_else(|e| e.into_inner()) = Some(Recorder {
        started: Instant::now(),
        formats,
        units: Vec::new(),
        target_dir: None,
        title: String::new(),
    });
}

/// The build's target directory and a title for the report, set by the builder.
pub fn set_build(target_dir: &Path, title: &str) {
    if let Some(recorder) = RECORDER.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        recorder.target_dir = Some(target_dir.to_path_buf());
        recorder.title = title.to_string();
    }
}

/// Writes the report(s) of a recording whose build set its target directory.
pub fn finish_recorded() -> Result<Vec<PathBuf>> {
    let (dir, title) = {
        let mut guard = RECORDER.lock().unwrap_or_else(|e| e.into_inner());
        match guard
            .as_ref()
            .and_then(|r| r.target_dir.clone().map(|d| (d, r.title.clone())))
        {
            Some(found) => found,
            None => {
                // No build ran (or it failed before choosing a target dir): stop recording.
                *guard = None;
                return Ok(Vec::new());
            }
        }
    };
    finish(&dir, &title)
}

/// Whether this command records unit timings.
pub fn is_enabled() -> bool {
    RECORDER.lock().unwrap_or_else(|e| e.into_inner()).is_some()
}

/// A unit's start, while recording; `None` otherwise.
pub fn unit_started() -> Option<Instant> {
    UNIT_NOTE.with(|n| n.set(UnitNote::default()));
    RECORDER
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map(|_| Instant::now())
}

/// Records a finished unit started at `started` (from [`unit_started`]); `succeeded` is
/// false for a unit that failed.
pub fn unit_finished(started: Option<Instant>, unit: String, succeeded: bool) {
    let note = UNIT_NOTE.with(|n| n.replace(UnitNote::default()));
    let Some(started) = started else {
        return;
    };
    let outcome = match (succeeded, note.restored) {
        (false, _) => "failed",
        (true, true) => "restored",
        (true, false) => "compiled",
    };
    let duration = started.elapsed();
    let mut guard = RECORDER.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(recorder) = guard.as_mut() {
        let start_secs = started
            .saturating_duration_since(recorder.started)
            .as_secs_f64();
        recorder.units.push(UnitTiming {
            unit,
            start_secs,
            duration_secs: duration.as_secs_f64(),
            outcome,
            link_secs: note.link.map(|d| d.as_secs_f64()),
        });
    }
}

/// Writes the report(s) under `target_dir/cargo-timings` and stops recording; returns the
/// files written (none when recording was off).
pub fn finish(target_dir: &Path, title: &str) -> Result<Vec<PathBuf>> {
    let Some(recorder) = RECORDER.lock().unwrap_or_else(|e| e.into_inner()).take() else {
        return Ok(Vec::new());
    };
    let total = recorder.started.elapsed().as_secs_f64();
    let mut units = recorder.units;
    units.sort_by(|a, b| a.start_secs.total_cmp(&b.start_secs));
    let dir = target_dir.join("cargo-timings");
    std::fs::create_dir_all(&dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut written = Vec::new();
    for format in &recorder.formats {
        let (ext, body) = match format {
            Format::Html => ("html", render_html(title, total, &units)),
            Format::Json => ("json", render_json(total, &units)),
        };
        let file = dir.join(format!("cargo-timing-{stamp}.{ext}"));
        std::fs::write(&file, &body).with_context(|| format!("cannot write {}", file.display()))?;
        let latest = dir.join(format!("cargo-timing.{ext}"));
        std::fs::write(&latest, &body)
            .with_context(|| format!("cannot write {}", latest.display()))?;
        written.push(file);
    }
    Ok(written)
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn render_html(title: &str, total: f64, units: &[UnitTiming]) -> String {
    let scale = if total > 0.0 { 100.0 / total } else { 0.0 };
    let mut rows = String::new();
    for (index, timing) in units.iter().enumerate() {
        let left = timing.start_secs * scale;
        let width = (timing.duration_secs * scale).max(0.2);
        let link = timing
            .link_secs
            .map(|s| format!("{s:.2}"))
            .unwrap_or_default();
        rows.push_str(&format!(
            "<tr><td>{}</td><td>{}</td><td class=\"num\">{:.2}</td><td class=\"num\">{:.2}</td>\
             <td class=\"num\">{link}</td><td>{}</td><td class=\"bar\"><span class=\"{}\" style=\"left:{left:.3}%;width:{width:.3}%\"></span></td></tr>\n",
            index + 1,
            escape(&timing.unit),
            timing.start_secs,
            timing.duration_secs,
            timing.outcome,
            timing.outcome,
        ));
    }
    let compiled = units.iter().filter(|u| u.outcome == "compiled").count();
    let restored = units.iter().filter(|u| u.outcome == "restored").count();
    format!(
        "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <title>Mitts build timings</title><style>\
         body{{font:14px system-ui,sans-serif;margin:16px;color:#1d1d1f;background:#fff}}\
         table{{border-collapse:collapse;width:100%}}th,td{{padding:4px 8px;border-bottom:1px solid #e5e5e5;text-align:left}}\
         .num{{text-align:right;font-variant-numeric:tabular-nums}}.bar{{position:relative;width:40%}}\
         .bar span{{position:absolute;top:6px;height:10px;border-radius:2px;background:#4a7bd0}}\
         .bar span.failed{{background:#c8473b}}.bar span.restored{{background:#9aa5b1}}\
         @media (prefers-color-scheme:dark){{body{{color:#e8e8e8;background:#161616}}th,td{{border-color:#333}}}}\
         </style></head><body>\n<h1>Mitts build timings</h1>\n<p>{} &middot; {} units ({compiled} compiled, {restored} restored from the cache) &middot; {:.2}s total</p>\n\
         <table><thead><tr><th>#</th><th>Unit</th><th class=\"num\">Start (s)</th><th class=\"num\">Duration (s)</th>\
         <th class=\"num\">Link (s)</th><th>Outcome</th><th>Timeline</th></tr></thead><tbody>\n{rows}</tbody></table>\n</body></html>\n",
        escape(title),
        units.len(),
        total
    )
}

fn render_json(total: f64, units: &[UnitTiming]) -> String {
    let units: Vec<serde_json::Value> = units
        .iter()
        .map(|t| {
            serde_json::json!({
                "unit": t.unit,
                "start": t.start_secs,
                "duration": t.duration_secs,
                "link": t.link_secs,
                "outcome": t.outcome,
            })
        })
        .collect();
    serde_json::to_string_pretty(&serde_json::json!({ "total": total, "units": units }))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_parse_like_cargo() {
        assert_eq!(parse_formats("").unwrap(), [Format::Html]);
        assert_eq!(
            parse_formats("json,html").unwrap(),
            [Format::Json, Format::Html]
        );
        assert!(parse_formats("xml").is_err());
    }

    #[test]
    fn reports_escape_names_and_list_units() {
        let units = [
            UnitTiming {
                unit: "a<b>".to_string(),
                start_secs: 0.5,
                duration_secs: 1.0,
                outcome: "compiled",
                link_secs: Some(0.25),
            },
            UnitTiming {
                unit: "dep".to_string(),
                start_secs: 0.0,
                duration_secs: 0.01,
                outcome: "restored",
                link_secs: None,
            },
        ];
        let html = render_html("build [dev]", 2.0, &units);
        assert!(html.contains("a&lt;b&gt;"));
        assert!(html.contains("2 units (1 compiled, 1 restored from the cache)"));
        assert!(html.contains("<td class=\"num\">0.25</td>"));
        let json: serde_json::Value = serde_json::from_str(&render_json(2.0, &units)).unwrap();
        assert_eq!(json["units"][0]["unit"], "a<b>");
        assert_eq!(json["units"][0]["link"], 0.25);
        assert!(json["units"][1]["link"].is_null());
        assert_eq!(json["units"][1]["outcome"], "restored");
    }

    #[test]
    fn a_unit_note_names_the_outcome_and_is_reset_per_unit() {
        // No recorder: the note is still consumed so it cannot leak into the next unit.
        note_restored();
        note_link(std::time::Duration::from_millis(5));
        unit_finished(None, "x".into(), true);
        UNIT_NOTE.with(|n| {
            let note = n.get();
            assert!(!note.restored && note.link.is_none());
        });
    }
}
