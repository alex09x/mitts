/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! How long linking took, separate from compiling. rustc runs the linker itself and
//! stable rustc reports no phase times, so with `-v` or `--timings` Mitts names itself
//! as the linker of the target's units (`-C linker=<mitts>`): the Mitts process started
//! that way runs the real linker with the same arguments, measures it, and appends the
//! time to a report file that the unit reads back once rustc has finished.
//!
//! Only C-compiler linker drivers (`cc`, `gcc`, `clang`, `*-gcc`, ...) are wrapped,
//! because rustc picks the linker flavor from the linker's file name and a driver name
//! resolves to the same flavor as the target default; a unit whose linker is set by
//! `RUSTFLAGS` or arguments after `--`, or whose linker is `lld`, `ld`, `wild` or
//! anything else, links exactly as before and reports no link time. The wrapper never
//! enters an action key: it changes who starts the linker, not what it links.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// The real linker, for a Mitts process started by rustc as the linker.
pub const REAL_LINKER_ENV: &str = "MITTS_LINK_TIMING_LINKER";
/// The file the wrapper appends each link's nanoseconds to.
pub const REPORT_ENV: &str = "MITTS_LINK_TIMING_REPORT";

static ENABLED: AtomicBool = AtomicBool::new(false);
static LINK_NANOS: AtomicU64 = AtomicU64::new(0);
static LINKED_UNITS: AtomicU64 = AtomicU64::new(0);
static REPORT_SEQ: AtomicU64 = AtomicU64::new(0);
static LINKERS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Measures link times for the rest of this command.
pub fn enable() {
    ENABLED.store(true, Ordering::Relaxed);
}

pub fn is_enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Whether rustc infers the same linker flavor for `linker` as for a Mitts wrapper:
/// a C compiler driver, possibly with a target prefix or a version suffix.
pub fn is_wrappable_driver(linker: &str) -> bool {
    let name = Path::new(linker)
        .file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let name = name.strip_suffix(".exe").unwrap_or(&name);
    // `aarch64-linux-gnu-gcc-13`: the last part that is not a version number.
    let stem = name
        .rsplit('-')
        .find(|part| !part.chars().all(|c| c.is_ascii_digit() || c == '.'))
        .unwrap_or("");
    let base = stem.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.');
    matches!(base, "cc" | "c++" | "gcc" | "g++" | "clang" | "clang++")
}

/// Points `cmd` at the wrapper when link timing is on and `linker` (the configured one,
/// or the platform's `cc`) can be wrapped. Returns false when the caller must apply the
/// configured linker itself.
pub fn wrap(cmd: &mut Command, configured: Option<&str>) -> bool {
    if !is_enabled() || cfg!(windows) {
        return false;
    }
    let real = configured.unwrap_or("cc");
    if !is_wrappable_driver(real) {
        return false;
    }
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let report = std::env::temp_dir().join(format!(
        "mitts-link-{}-{}",
        std::process::id(),
        REPORT_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_file(&report);
    cmd.args(["-C", &format!("linker={}", exe.display())]);
    cmd.env(REAL_LINKER_ENV, real);
    cmd.env(REPORT_ENV, &report);
    let mut linkers = LINKERS.lock().unwrap_or_else(|e| e.into_inner());
    if !linkers.iter().any(|l| l == real) {
        linkers.push(real.to_string());
    }
    true
}

/// The report file of a command that [`wrap`] pointed at the wrapper.
pub fn report_of(cmd: &Command) -> Option<PathBuf> {
    cmd.get_envs()
        .find(|(key, _)| *key == REPORT_ENV)
        .and_then(|(_, value)| value.map(PathBuf::from))
}

/// Reads and removes a unit's report; adds its link time to this command's total.
pub fn collect(report: &Path) -> Option<Duration> {
    let text = std::fs::read_to_string(report).ok();
    let _ = std::fs::remove_file(report);
    let nanos: u64 = text?
        .lines()
        .filter_map(|line| line.trim().parse::<u64>().ok())
        .sum();
    LINK_NANOS.fetch_add(nanos, Ordering::Relaxed);
    LINKED_UNITS.fetch_add(1, Ordering::Relaxed);
    Some(Duration::from_nanos(nanos))
}

/// Forgets the link times and linkers of an earlier build in this process.
pub fn reset() {
    LINK_NANOS.store(0, Ordering::Relaxed);
    LINKED_UNITS.store(0, Ordering::Relaxed);
    LINKERS.lock().unwrap_or_else(|e| e.into_inner()).clear();
}

/// Total link time, linked units, and the linkers that were wrapped.
pub fn snapshot() -> (Duration, u64, Vec<String>) {
    (
        Duration::from_nanos(LINK_NANOS.load(Ordering::Relaxed)),
        LINKED_UNITS.load(Ordering::Relaxed),
        LINKERS.lock().unwrap_or_else(|e| e.into_inner()).clone(),
    )
}

/// When this process was started by rustc as the linker: runs the real linker, records
/// its time and returns its exit code. `None` for every ordinary Mitts invocation.
pub fn run_as_linker_if_requested() -> Option<i32> {
    let real = std::env::var_os(REAL_LINKER_ENV)?;
    let report = std::env::var_os(REPORT_ENV)?;
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let start = Instant::now();
    let status = Command::new(&real)
        .args(&args)
        .env_remove(REAL_LINKER_ENV)
        .env_remove(REPORT_ENV)
        .status();
    let nanos = start.elapsed().as_nanos();
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&report)
    {
        use std::io::Write;
        let _ = writeln!(file, "{nanos}");
    }
    Some(match status {
        Ok(status) => status.code().unwrap_or(1),
        Err(error) => {
            eprintln!(
                "error: Mitts could not start the linker `{}`: {error}",
                Path::new(&real).display()
            );
            1
        }
    })
}

/// `<linker> --version`'s first line, for the build summary.
pub fn linker_version(linker: &str) -> Option<String> {
    let output = Command::new(linker).arg("--version").output().ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_c_compiler_drivers_are_wrapped() {
        for driver in [
            "cc",
            "/usr/bin/gcc",
            "clang",
            "clang-19",
            "riscv64-linux-gnu-gcc",
            "aarch64-linux-gnu-gcc-13",
            "x86_64-w64-mingw32-gcc.exe",
            "c++",
            "clang++",
        ] {
            assert!(is_wrappable_driver(driver), "{driver}");
        }
        for other in [
            "rust-lld", "ld.lld", "ld", "wild", "mold", "link.exe", "lld-link", "",
        ] {
            assert!(!is_wrappable_driver(other), "{other}");
        }
    }

    #[test]
    fn reports_sum_every_link_of_a_unit() {
        let dir = tempfile::tempdir().unwrap();
        let report = dir.path().join("r");
        std::fs::write(&report, "1000000\n2000000\n").unwrap();
        assert_eq!(collect(&report), Some(Duration::from_millis(3)));
        assert!(!report.exists());
        assert_eq!(collect(&report), None);
    }
}
