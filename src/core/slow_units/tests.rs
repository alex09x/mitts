/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::*;
use std::sync::Arc;

/// The registry is process-wide: tests that start units run one at a time.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

#[test]
fn thresholds_parse_like_durations() {
    let s = Duration::from_secs;
    assert_eq!(parse_threshold("30s").unwrap(), Some(s(30)));
    assert_eq!(parse_threshold("10m").unwrap(), Some(s(600)));
    assert_eq!(parse_threshold("1h").unwrap(), Some(s(3600)));
    assert_eq!(parse_threshold("1h30m").unwrap(), Some(s(5400)));
    assert_eq!(parse_threshold("2m5s").unwrap(), Some(s(125)));
    assert_eq!(parse_threshold("90").unwrap(), Some(s(90)));
    assert_eq!(parse_threshold(" 5M ").unwrap(), Some(s(300)));
    assert_eq!(parse_threshold("off").unwrap(), None);
    assert_eq!(parse_threshold("0").unwrap(), None);
    assert_eq!(parse_threshold("0m").unwrap(), None);
    for bad in ["", "m", "10x", "5m3", "1.5m", "-3s", "abc"] {
        assert!(parse_threshold(bad).is_err(), "{bad} should be rejected");
    }
}

#[test]
fn the_flag_wins_over_the_variable_and_the_default_is_five_minutes() {
    let _serial = serial();
    configure(None, None).unwrap();
    assert_eq!(threshold(), Some(DEFAULT_THRESHOLD));
    configure(None, Some("")).unwrap();
    assert_eq!(threshold(), Some(DEFAULT_THRESHOLD));
    configure(None, Some("2m")).unwrap();
    assert_eq!(threshold(), Some(Duration::from_secs(120)));
    configure(Some("off"), Some("2m")).unwrap();
    assert_eq!(threshold(), None);
    let err = configure(None, Some("soon")).unwrap_err().to_string();
    assert!(err.contains("MITTS_SLOW_UNITS"), "{err}");
    let err = configure(Some("1y"), None).unwrap_err().to_string();
    assert!(err.contains("--slow-units"), "{err}");
    configure(None, None).unwrap();
}

#[test]
fn elapsed_times_read_like_a_clock() {
    let s = Duration::from_secs;
    assert_eq!(format_elapsed(Duration::from_millis(900)), "0s");
    assert_eq!(format_elapsed(s(42)), "42s");
    assert_eq!(format_elapsed(s(60)), "1m00s");
    assert_eq!(format_elapsed(s(662)), "11m02s");
    assert_eq!(format_elapsed(s(3780)), "1h03m");
    assert_eq!(format_elapsed(s(4 * 3600 + 59)), "4h00m");
}

#[test]
fn the_progress_line_names_the_longest_running_first() {
    let s = Duration::from_secs;
    let running = vec![
        ("melodia_ui".to_string(), s(662)),
        ("melodia_views".to_string(), s(220)),
        ("zbus".to_string(), s(3)),
        ("x11rb".to_string(), s(2)),
        ("syn".to_string(), s(1)),
    ];
    assert_eq!(
        running_message(&running, 3),
        "• melodia_ui 11m02s, melodia_views 3m40s, zbus 3s, +2 more"
    );
    assert_eq!(running_message(&running[..1], 3), "• melodia_ui 11m02s");
}

#[test]
fn a_slow_unit_is_announced_once_per_threshold_with_its_command_first() {
    let _serial = serial();
    let threshold = Duration::from_millis(40);
    let guard = start(
        "slowpkg slowpkg (lib)".into(),
        "rustc --crate-name slowpkg src/lib.rs".into(),
        "codegen-units=1".into(),
    );
    assert!(due_notices(threshold).is_empty(), "not slow yet");
    std::thread::sleep(Duration::from_millis(50));
    let first = due_notices(threshold);
    assert_eq!(first.len(), 1, "{first:?}");
    assert!(
        first[0].starts_with("⏳ slowpkg slowpkg (lib) still running after 0s (codegen-units=1)")
    );
    assert!(first[0].contains("\n   rustc --crate-name slowpkg src/lib.rs"));
    assert!(
        due_notices(threshold).is_empty(),
        "announced once per multiple"
    );
    std::thread::sleep(Duration::from_millis(45));
    let second = due_notices(threshold);
    assert_eq!(second.len(), 1, "{second:?}");
    assert!(
        !second[0].contains("rustc"),
        "the command is printed once: {second:?}"
    );
    assert!(!second[0].contains("codegen-units"), "{second:?}");
    assert!(running().iter().any(|(l, _)| l == "slowpkg slowpkg (lib)"));
    drop(guard);
    assert!(!running().iter().any(|(l, _)| l == "slowpkg slowpkg (lib)"));
}

#[test]
fn phases_split_the_time_into_front_end_and_code_generation() {
    let s = Duration::from_secs;
    assert_eq!(phase_text(s(600), false, None), None);
    assert_eq!(
        phase_text(s(600), true, None).as_deref(),
        Some("still in the front end (parsing, macros, type checking)")
    );
    assert_eq!(
        phase_text(s(600), true, Some(s(70))).as_deref(),
        Some("front end 1m10s, code generation 8m50s")
    );
}

#[test]
fn a_unit_with_phases_says_which_phase_it_is_in() {
    let _serial = serial();
    let threshold = Duration::from_millis(40);
    let guard = start(
        "lib lib (lib)".into(),
        "rustc --crate-name lib".into(),
        "codegen-units=1".into(),
    );
    guard.track_phases();
    std::thread::sleep(Duration::from_millis(50));
    let first = due_notices(threshold);
    assert_eq!(first.len(), 1, "{first:?}");
    assert!(
        first[0].starts_with(
            "⏳ lib lib (lib) still running after 0s: still in the front end \
             (parsing, macros, type checking) (codegen-units=1)"
        ),
        "{first:?}"
    );
    guard.front_end_done();
    std::thread::sleep(Duration::from_millis(45));
    let second = due_notices(threshold);
    assert_eq!(second.len(), 1, "{second:?}");
    assert!(
        second[0].contains(": front end 0s, code generation 0s"),
        "{second:?}"
    );
    // A second report of the front end's end keeps the first time.
    guard.front_end_done();
    drop(guard);
}

#[test]
fn a_fast_unit_is_never_announced_and_the_summary_lists_the_slowest() {
    let _serial = serial();
    reset();
    drop(start(
        "fast (lib)".into(),
        "rustc fast".into(),
        String::new(),
    ));
    let slow = start("slow (lib)".into(), "rustc slow".into(), String::new());
    std::thread::sleep(Duration::from_millis(30));
    assert!(due_notices(Duration::from_secs(60)).is_empty());
    drop(slow);
    assert_eq!(slowest_line(Duration::from_secs(60)), None);
    let line = slowest_line(Duration::ZERO).unwrap();
    assert!(
        line.starts_with("slowest: slow (lib) 0s · fast (lib) 0s"),
        "{line}"
    );
    reset();
    assert_eq!(slowest_line(Duration::ZERO), None);
}

#[test]
fn the_watcher_updates_the_progress_line_and_prints_slow_units_until_stopped() {
    let _serial = serial();
    configure(Some("1s"), None).unwrap();
    let seen: Arc<Mutex<Vec<Notice>>> = Arc::default();
    let sink_seen = seen.clone();
    let guard =
        watch(Arc::new(move |n| sink_seen.lock().unwrap().push(n))).expect("no other watcher runs");
    assert!(watch(Arc::new(|_| {})).is_none(), "one watcher at a time");
    let unit = start(
        "watched (bin)".into(),
        "rustc --crate-name watched".into(),
        "lto=fat".into(),
    );
    std::thread::sleep(Duration::from_millis(1500));
    drop(unit);
    drop(guard);
    let after_stop = seen.lock().unwrap().len();
    drop(start("later (lib)".into(), "rustc".into(), String::new()));
    std::thread::sleep(Duration::from_millis(300));
    configure(None, None).unwrap();

    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), after_stop, "nothing after the guard is dropped");
    assert!(
        seen.iter()
            .any(|n| matches!(n, Notice::Running(m) if m.starts_with("• watched (bin) "))),
        "{seen:?}"
    );
    let lines: Vec<&String> = seen
        .iter()
        .filter_map(|n| match n {
            Notice::Line(l) => Some(l),
            Notice::Running(_) => None,
        })
        .collect();
    assert!(
        lines.iter().any(
            |l| l.starts_with("⏳ watched (bin) still running after 1s (lto=fat)")
                && l.contains("rustc --crate-name watched")
        ),
        "{lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("✓ watched (bin) finished in 1s")),
        "{lines:?}"
    );
}

#[test]
fn notices_off_still_refresh_the_progress_line() {
    let _serial = serial();
    configure(Some("off"), None).unwrap();
    assert_eq!(watch::tick(threshold()), Duration::from_secs(1));
    assert_eq!(
        watch::tick(Some(Duration::from_secs(1))),
        Duration::from_millis(200)
    );
    assert_eq!(
        watch::tick(Some(Duration::from_millis(100))),
        Duration::from_millis(50)
    );
    let seen: Arc<Mutex<Vec<Notice>>> = Arc::default();
    let sink_seen = seen.clone();
    let guard = watch(Arc::new(move |n| sink_seen.lock().unwrap().push(n))).unwrap();
    let unit = start("quiet (lib)".into(), "rustc".into(), String::new());
    std::thread::sleep(Duration::from_millis(1300));
    drop(unit);
    drop(guard);
    configure(None, None).unwrap();
    let seen = seen.lock().unwrap();
    assert!(
        seen.iter().any(|n| matches!(n, Notice::Running(_))),
        "{seen:?}"
    );
    assert!(
        !seen.iter().any(|n| matches!(n, Notice::Line(_))),
        "{seen:?}"
    );
}
