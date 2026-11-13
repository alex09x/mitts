/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! `--slow-units` / `MITTS_SLOW_UNITS` (issue #109 follow-up): a compiler or build-script
//! process still running after the threshold is reported with its command, again at
//! every further multiple, and named in the summary; link time is shown without `-v`.
//! A proc macro that sleeps makes a unit slow on every machine.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use tempfile::TempDir;

fn write_file(path: impl AsRef<Path>, contents: &str) {
    let path = path.as_ref();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// `app` (a binary) expands `slowmac::slow!(ms)`, which sleeps `ms` inside rustc.
fn slow_macro_project(ms: u64) -> TempDir {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    write_file(
        root.join("Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\
         [dependencies]\nslowmac = { path = \"slowmac\" }\n",
    );
    write_file(
        root.join("src/main.rs"),
        &format!("slowmac::slow!({ms});\nfn main() {{ slow_done(); }}\n"),
    );
    write_file(
        root.join("slowmac/Cargo.toml"),
        "[package]\nname = \"slowmac\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\
         [lib]\nproc-macro = true\n",
    );
    write_file(
        root.join("slowmac/src/lib.rs"),
        "use proc_macro::TokenStream;\n\
         #[proc_macro]\npub fn slow(input: TokenStream) -> TokenStream {\n    \
         let ms: u64 = input.to_string().trim().parse().unwrap();\n    \
         std::thread::sleep(std::time::Duration::from_millis(ms));\n    \
         \"fn slow_done() {}\".parse().unwrap()\n}\n",
    );
    temp
}

/// A binary whose build script sleeps `ms`.
fn slow_build_script_project(ms: u64) -> TempDir {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    write_file(
        root.join("Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write_file(root.join("src/main.rs"), "fn main() {}\n");
    write_file(
        root.join("build.rs"),
        &format!(
            "fn main() {{\n    std::thread::sleep(std::time::Duration::from_millis({ms}));\n    \
             println!(\"cargo:rerun-if-changed=build.rs\");\n}}\n"
        ),
    );
    temp
}

fn mitts_with(root: &Path, args: &[&str], envs: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mitts"));
    cmd.args(args)
        .arg("--cache-dir")
        .arg(root.join("mitts-cache"))
        .current_dir(root)
        .env("RUSTC_WRAPPER", "")
        .env("MITTS_FAST_LOCK", "0")
        .env_remove("MITTS_VERBOSE")
        .env_remove("MITTS_SLOW_UNITS")
        .env_remove("MITTS_LINK_TIMING")
        .env_remove("RUSTFLAGS")
        .env_remove("CARGO_TARGET_DIR")
        .env_remove("CARGO_BUILD_TARGET_DIR");
    for (key, value) in envs {
        cmd.env(key, value);
    }
    cmd.output().expect("run Mitts")
}

fn ok(output: Output) -> (String, String) {
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        output.status.success(),
        "stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    (stdout, stderr)
}

/// The `⏳` notices, each with the command line that follows its first one.
fn notices(stderr: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in stderr.lines() {
        if line.starts_with("⏳ ") {
            out.push(line.to_string());
        } else if line.starts_with("   ") && !out.is_empty() && !line.trim().is_empty() {
            let last = out.last_mut().unwrap();
            last.push('\n');
            last.push_str(line);
        }
    }
    out
}

fn finished_secs(stderr: &str, label: &str) -> u64 {
    let prefix = format!("✓ {label} finished in ");
    let line = stderr
        .lines()
        .find(|l| l.starts_with(&prefix))
        .unwrap_or_else(|| panic!("no `{prefix}` in:\n{stderr}"));
    line[prefix.len()..]
        .trim_end_matches('s')
        .parse()
        .unwrap_or_else(|_| panic!("{line}"))
}

#[test]
fn a_unit_past_the_threshold_is_reported_with_its_command_then_reminded() {
    let temp = slow_macro_project(2500);
    let (stdout, stderr) = ok(mitts_with(temp.path(), &["build", "--slow-units=1s"], &[]));
    let app: Vec<String> = notices(&stderr)
        .into_iter()
        .filter(|n| n.starts_with("⏳ app app (bin) "))
        .collect();
    assert!(app.len() >= 2, "a notice at 1s and at 2s:\n{stderr}");
    assert!(
        app[0].starts_with("⏳ app app (bin) still running after 1s"),
        "{stderr}"
    );
    assert!(app[0].contains("--crate-name app"), "the command: {stderr}");
    assert!(
        app[1].starts_with("⏳ app app (bin) still running after 2s"),
        "{stderr}"
    );
    assert!(
        app[1..].iter().all(|n| !n.contains("--crate-name")),
        "the command is printed once:\n{stderr}"
    );
    if cfg!(unix) {
        // The command can be run again: the link-timing wrapper is shown as `cc`.
        assert!(app[0].contains("linker=cc"), "{}", app[0]);
        let exe = env!("CARGO_BIN_EXE_mitts");
        assert!(!app[0].contains(&format!("linker={exe}")), "{}", app[0]);
    }
    // Whether the proc macro itself is reported depends on the machine: under qemu its
    // own compile takes more than a second. The 10s case of
    // `the_threshold_decides_when_the_first_notice_comes` covers units that stay quiet.
    assert!(finished_secs(&stderr, "app app (bin)") >= 2, "{stderr}");
    let slowest = stdout
        .lines()
        .find(|l| l.contains("slowest: "))
        .unwrap_or_else(|| panic!("no slowest line:\n{stdout}"));
    assert!(slowest.contains("slowest: app app (bin) "), "{slowest}");
    // The slow unit still builds a working program.
    let exe = temp
        .path()
        .join("target/debug")
        .join(format!("app{}", std::env::consts::EXE_SUFFIX));
    assert!(Command::new(exe).status().unwrap().success());
}

#[test]
fn the_threshold_decides_when_the_first_notice_comes() {
    for (threshold, first) in [
        ("2s", Some("after 2s")),
        ("2", Some("after 2s")),
        ("10s", None),
    ] {
        let temp = slow_macro_project(2500);
        let flag = format!("--slow-units={threshold}");
        let (stdout, stderr) = ok(mitts_with(temp.path(), &["build", &flag], &[]));
        let app: Vec<String> = notices(&stderr)
            .into_iter()
            .filter(|n| n.starts_with("⏳ app app (bin) "))
            .collect();
        match first {
            Some(after) => {
                assert!(
                    app.first().is_some_and(|n| n.contains(after)),
                    "{threshold}: {stderr}"
                );
                assert!(app[0].contains("--crate-name app"), "{threshold}: {stderr}");
                assert!(!stderr.contains("after 1s"), "{threshold}: {stderr}");
                assert!(stdout.contains("slowest: app app (bin) "), "{stdout}");
            }
            None => {
                assert!(app.is_empty(), "{threshold}: {stderr}");
                assert!(!stderr.contains("finished in"), "{threshold}: {stderr}");
                assert!(!stdout.contains("slowest: "), "{threshold}: {stdout}");
            }
        }
    }
}

#[test]
fn the_default_threshold_of_five_minutes_keeps_quick_builds_quiet() {
    let temp = slow_macro_project(1500);
    let (stdout, stderr) = ok(mitts_with(temp.path(), &["build"], &[]));
    assert!(notices(&stderr).is_empty(), "{stderr}");
    assert!(!stdout.contains("slowest: "), "{stdout}");
}

#[test]
fn the_variable_sets_the_threshold_and_the_flag_wins_over_it() {
    let temp = slow_macro_project(1500);
    let (_, stderr) = ok(mitts_with(
        temp.path(),
        &["build"],
        &[("MITTS_SLOW_UNITS", "1s")],
    ));
    assert!(
        notices(&stderr)
            .iter()
            .any(|n| n.starts_with("⏳ app app (bin) still running after 1s")),
        "{stderr}"
    );

    let temp = slow_macro_project(1500);
    let (stdout, stderr) = ok(mitts_with(
        temp.path(),
        &["build", "--slow-units=off"],
        &[("MITTS_SLOW_UNITS", "1s")],
    ));
    assert!(notices(&stderr).is_empty(), "{stderr}");
    assert!(!stdout.contains("slowest: "), "{stdout}");
}

/// A library whose macro expansion sleeps: rustc is in its front end while the notices
/// come, and the finished line splits front end from code generation (the `.rmeta`).
#[test]
fn a_slow_library_reports_its_phase() {
    let temp = slow_macro_project(0);
    let root = temp.path();
    write_file(
        root.join("Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\
         [dependencies]\nslowlib = { path = \"slowlib\" }\n",
    );
    write_file(root.join("src/main.rs"), "fn main() { slowlib::f(); }\n");
    write_file(
        root.join("slowlib/Cargo.toml"),
        "[package]\nname = \"slowlib\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\
         [dependencies]\nslowmac = { path = \"../slowmac\" }\n",
    );
    write_file(
        root.join("slowlib/src/lib.rs"),
        "slowmac::slow!(2500);\npub fn f() { slow_done(); }\n",
    );
    let (stdout, stderr) = ok(mitts_with(root, &["build", "--slow-units=1s"], &[]));
    let lib: Vec<String> = notices(&stderr)
        .into_iter()
        .filter(|n| n.starts_with("⏳ slowlib slowlib (lib) "))
        .collect();
    assert!(
        lib.first().is_some_and(|n| n.starts_with(
            "⏳ slowlib slowlib (lib) still running after 1s: \
             still in the front end (parsing, macros, type checking)"
        )),
        "{stderr}"
    );
    assert!(lib[0].contains("--crate-name slowlib"), "{stderr}");
    let finished = stderr
        .lines()
        .find(|l| l.starts_with("✓ slowlib slowlib (lib) finished in "))
        .unwrap_or_else(|| panic!("{stderr}"));
    // The sleep is in the front end: at least 2 s of it, before code generation.
    let front: u64 = finished
        .split("(front end ")
        .nth(1)
        .and_then(|rest| rest.split('s').next())
        .and_then(|secs| secs.parse().ok())
        .unwrap_or_else(|| panic!("{finished}"));
    assert!(front >= 2, "{finished}");
    assert!(finished.contains(", code generation "), "{finished}");
    assert!(stdout.contains("slowlib slowlib (lib) "), "{stdout}");
    // A binary reports no phases: it emits no `.rmeta`. (Under emulation even this tiny
    // binary can pass the 1s threshold.)
    assert!(
        notices(&stderr)
            .iter()
            .filter(|n| n.starts_with("⏳ app app (bin) "))
            .all(|n| !n.contains("front end")),
        "{stderr}"
    );
}

/// A library that is also a cdylib emits no `.rmeta` (review of #127): it reports no
/// phase rather than "still in the front end" through its whole codegen.
#[test]
fn a_cdylib_library_reports_no_phase() {
    let temp = slow_macro_project(0);
    let root = temp.path();
    write_file(
        root.join("Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\
         [dependencies]\nslowlib = { path = \"slowlib\" }\n",
    );
    write_file(root.join("src/main.rs"), "fn main() { slowlib::f(); }\n");
    write_file(
        root.join("slowlib/Cargo.toml"),
        "[package]\nname = \"slowlib\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\
         [lib]\ncrate-type = [\"lib\", \"cdylib\"]\n\
         [dependencies]\nslowmac = { path = \"../slowmac\" }\n",
    );
    write_file(
        root.join("slowlib/src/lib.rs"),
        "slowmac::slow!(1800);\npub fn f() { slow_done(); }\n",
    );
    let (_, stderr) = ok(mitts_with(root, &["build", "--slow-units=1s"], &[]));
    let lib: Vec<String> = notices(&stderr)
        .into_iter()
        .filter(|n| n.starts_with("⏳ slowlib slowlib (lib,cdylib) "))
        .collect();
    assert!(!lib.is_empty(), "{stderr}");
    assert!(
        lib[0].starts_with("⏳ slowlib slowlib (lib,cdylib) still running after 1s\n"),
        "no phase, then the command: {stderr}"
    );
    assert!(lib.iter().all(|n| !n.contains("front end")), "{stderr}");
    let finished = stderr
        .lines()
        .find(|l| l.starts_with("✓ slowlib slowlib (lib,cdylib) finished in "))
        .unwrap_or_else(|| panic!("{stderr}"));
    assert!(!finished.contains("front end"), "{finished}");
}

/// `mitts test` and `mitts run` with stderr piped (review of #127): the hidden progress
/// bar must not swallow the slow-unit notices or the stats line.
#[test]
fn test_and_run_print_notices_when_stderr_is_piped() {
    for command in ["test", "run"] {
        let temp = slow_macro_project(1800);
        let (_, stderr) = ok(mitts_with(temp.path(), &[command, "--slow-units=1s"], &[]));
        assert!(
            notices(&stderr)
                .iter()
                .any(|n| n.starts_with("⏳ app app ") && n.contains("still running after 1s")),
            "{command}: {stderr}"
        );
        assert!(stderr.contains("[mitts::stats] "), "{command}: {stderr}");
    }
}

#[test]
fn a_slow_build_script_is_reported() {
    let temp = slow_build_script_project(1800);
    let (_, stderr) = ok(mitts_with(temp.path(), &["build", "--slow-units=1s"], &[]));
    let script: Vec<String> = notices(&stderr)
        .into_iter()
        .filter(|n| n.starts_with("⏳ app build script (run) still running after 1s"))
        .collect();
    assert_eq!(script.len(), 1, "{stderr}");
    assert!(
        script[0].contains("build_script_build"),
        "the command: {stderr}"
    );
    assert!(
        finished_secs(&stderr, "app build script (run)") >= 1,
        "{stderr}"
    );
}

#[test]
fn check_reports_slow_units_too() {
    let temp = slow_macro_project(1800);
    let (_, stderr) = ok(mitts_with(temp.path(), &["check", "--slow-units=1s"], &[]));
    assert!(
        notices(&stderr)
            .iter()
            .any(|n| n.starts_with("⏳ app app (bin) still running after 1s")),
        "{stderr}"
    );
}

#[test]
fn quiet_builds_and_cache_hits_report_nothing() {
    let temp = slow_macro_project(1500);
    let (_, stderr) = ok(mitts_with(
        temp.path(),
        &["build", "-q", "--slow-units=1s"],
        &[],
    ));
    assert!(notices(&stderr).is_empty(), "-q: {stderr}");
    assert!(!stderr.contains("finished in"), "-q: {stderr}");
    // Everything comes from the cache now: no process runs, nothing is slow.
    let (stdout, stderr) = ok(mitts_with(temp.path(), &["build", "--slow-units=1s"], &[]));
    assert!(notices(&stderr).is_empty(), "cached: {stderr}");
    assert!(!stdout.contains("slowest: "), "cached: {stdout}");
}

#[test]
fn invalid_thresholds_are_rejected() {
    let temp = slow_build_script_project(0);
    let out = mitts_with(temp.path(), &["build", "--slow-units=soon"], &[]);
    assert_eq!(out.status.code(), Some(101));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--slow-units: invalid duration `soon`"),
        "{stderr}"
    );
    let out = mitts_with(temp.path(), &["build"], &[("MITTS_SLOW_UNITS", "1.5m")]);
    assert_eq!(out.status.code(), Some(101));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("MITTS_SLOW_UNITS: invalid duration"),
        "{stderr}"
    );
}

#[test]
fn link_time_and_the_linker_are_shown_without_verbose() {
    let temp = slow_build_script_project(0);
    let (stdout, _) = ok(mitts_with(temp.path(), &["build"], &[]));
    if cfg!(unix) {
        assert!(stdout.contains("linker: cc"), "{stdout}");
        let time = stdout.lines().find(|l| l.contains("time: ")).unwrap();
        assert!(time.contains(", link "), "{time}");
    }
    let exe = temp
        .path()
        .join("target/debug")
        .join(format!("app{}", std::env::consts::EXE_SUFFIX));
    assert!(Command::new(&exe).status().unwrap().success());

    let temp = slow_build_script_project(0);
    let (stdout, _) = ok(mitts_with(
        temp.path(),
        &["build"],
        &[("MITTS_LINK_TIMING", "0")],
    ));
    assert!(!stdout.contains("linker: "), "{stdout}");
    assert!(!stdout.contains(", link "), "{stdout}");
}

#[test]
fn verbose_names_the_slowest_units_of_any_build() {
    let temp = slow_build_script_project(0);
    let (stdout, stderr) = ok(mitts_with(temp.path(), &["build", "-v"], &[]));
    assert!(stdout.contains("slowest: "), "{stdout}");
    assert!(notices(&stderr).is_empty(), "{stderr}");
}
