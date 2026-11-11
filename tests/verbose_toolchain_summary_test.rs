/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! The build summary, `-v`/`-vv`, link timing and `+toolchain` (issue #109 follow-up).
//! A rebuild after one edit must say how many units were compiled and how many came
//! from the cache, never count restored units as compiled.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use tempfile::TempDir;

fn write_file(path: impl AsRef<Path>, contents: &str) {
    let path = path.as_ref();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// A binary with a library dependency (two units); the binary has a build script.
fn project() -> TempDir {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    write_file(
        root.join("Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\
         [dependencies]\nhelper = { path = \"helper\" }\n",
    );
    write_file(root.join("src/main.rs"), "fn main() { helper::h(); }\n");
    write_file(
        root.join("build.rs"),
        "fn main() {\n    println!(\"hello-from-build-script\");\n    \
         eprintln!(\"stderr-from-build-script\");\n    \
         println!(\"cargo:rerun-if-changed=build.rs\");\n}\n",
    );
    write_file(
        root.join("helper/Cargo.toml"),
        "[package]\nname = \"helper\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write_file(root.join("helper/src/lib.rs"), "pub fn h() {}\n");
    temp
}

fn mitts_with(root: &Path, args: &[&str], envs: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mitts"));
    cmd.args(args)
        .arg("--cache-dir")
        .arg(root.join("mitts-cache"))
        .current_dir(root)
        // A machine-wide sccache or a fast-lock answer would change what is compiled.
        .env("RUSTC_WRAPPER", "")
        .env("MITTS_FAST_LOCK", "0")
        .env_remove("MITTS_VERBOSE")
        .env_remove("RUSTFLAGS")
        .env_remove("CARGO_TARGET_DIR")
        .env_remove("CARGO_BUILD_TARGET_DIR");
    for (key, value) in envs {
        cmd.env(key, value);
    }
    cmd.output().expect("run Mitts")
}

fn mitts(root: &Path, args: &[&str]) -> Output {
    mitts_with(root, args, &[])
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

fn edit_main(root: &Path) {
    write_file(
        root.join("src/main.rs"),
        "fn main() { helper::h(); let _x = 1; }\n",
    );
}

#[test]
fn a_rebuild_counts_compiled_and_cached_units_apart() {
    let temp = project();
    let root = temp.path();
    let (first, _) = ok(mitts(root, &["build"]));
    assert!(first.contains("2 compiled"), "{first}");
    edit_main(root);
    let (second, _) = ok(mitts(root, &["build"]));
    assert!(second.contains("1 compiled, 1 from the cache"), "{second}");
    assert!(!second.contains("actions executed"), "{second}");
    assert!(!second.contains("2 compiled"), "{second}");
    assert!(second.contains("toolchain: rustc "), "{second}");
    assert!(second.contains("time: "), "{second}");
    assert!(
        second.contains("rustc ") && second.contains("over 1 unit"),
        "{second}"
    );
    assert!(second.contains("cache restore "), "{second}");
}

#[test]
fn check_counts_checked_and_cached_units_apart() {
    let temp = project();
    let root = temp.path();
    ok(mitts(root, &["check"]));
    edit_main(root);
    let (stdout, _) = ok(mitts(root, &["check"]));
    assert!(stdout.contains("1 checked, 1 from the cache"), "{stdout}");
}

#[test]
fn verbose_prints_compiler_commands_cache_hits_and_the_linker() {
    let temp = project();
    let root = temp.path();
    let (_, first) = ok(mitts(root, &["build", "-v"]));
    assert!(first.contains("Running `"), "{first}");
    assert!(first.contains("--crate-name helper"), "{first}");
    assert!(first.contains("--crate-name app"), "{first}");
    // -v alone shows commands without their environment.
    assert!(!first.contains("CARGO_PKG_NAME=helper"), "{first}");
    edit_main(root);
    let (stdout, stderr) = ok(mitts(root, &["build", "-v"]));
    assert!(stderr.contains("Fresh helper v0.1.0"), "{stderr}");
    assert!(!stderr.contains("--crate-name helper"), "{stderr}");
    assert!(stderr.contains("--crate-name app"), "{stderr}");
    assert!(stdout.contains("(selected by "), "{stdout}");
    if cfg!(unix) {
        assert!(stdout.contains("linker: cc"), "{stdout}");
        assert!(stdout.contains("link "), "{stdout}");
    }
    // The binary linked through the timing wrapper runs.
    let exe = root
        .join("target/debug")
        .join(format!("app{}", std::env::consts::EXE_SUFFIX));
    assert!(Command::new(exe).status().unwrap().success());
}

#[test]
fn very_verbose_adds_environment_and_build_script_output() {
    let temp = project();
    let root = temp.path();
    let (_, stderr) = ok(mitts(root, &["build", "-vv"]));
    assert!(stderr.contains("CARGO_PKG_NAME=helper"), "{stderr}");
    assert!(
        stderr.contains("[app 0.1.0] hello-from-build-script"),
        "{stderr}"
    );
    assert!(
        stderr.contains("[app 0.1.0] stderr-from-build-script"),
        "{stderr}"
    );
    let temp = project();
    let (_, plain) = ok(mitts(temp.path(), &["build", "-v"]));
    assert!(!plain.contains("hello-from-build-script"), "{plain}");
}

#[test]
fn mitts_verbose_sets_the_level_when_no_flag_is_given() {
    let temp = project();
    let root = temp.path();
    let (_, stderr) = ok(mitts_with(root, &["build"], &[("MITTS_VERBOSE", "2")]));
    assert!(
        stderr.contains("[app 0.1.0] hello-from-build-script"),
        "{stderr}"
    );
}

#[test]
fn timings_mark_restored_units_and_link_times() {
    let temp = project();
    let root = temp.path();
    ok(mitts(root, &["build", "-q"]));
    edit_main(root);
    ok(mitts(root, &["build", "-q", "--timings=json"]));
    let json: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(root.join("target/cargo-timings/cargo-timing.json")).unwrap(),
    )
    .unwrap();
    let units = json["units"].as_array().unwrap();
    let unit = |name: &str| {
        units
            .iter()
            .find(|u| u["unit"].as_str().unwrap().starts_with(name))
            .unwrap_or_else(|| panic!("{name} in {json}"))
    };
    assert_eq!(unit("helper helper (lib)")["outcome"], "restored", "{json}");
    assert_eq!(unit("app app (bin)")["outcome"], "compiled", "{json}");
    if cfg!(unix) {
        assert!(
            unit("app app (bin)")["link"].as_f64().unwrap() > 0.0,
            "{json}"
        );
    }
    assert!(unit("helper helper (lib)")["link"].is_null(), "{json}");
}

#[test]
fn a_linker_chosen_in_rustflags_is_not_wrapped() {
    let temp = project();
    let root = temp.path();
    let (stdout, _) = ok(mitts_with(
        root,
        &["build", "-v"],
        &[("RUSTFLAGS", "-Clinker=cc")],
    ));
    assert!(!stdout.contains("linker: "), "{stdout}");
    assert!(!stdout.contains(", link "), "{stdout}");
}

/// `check` and `clippy` link nothing, so `-v` must not put the timing wrapper on their
/// commands or name a linker (review of #122).
#[test]
fn check_does_not_wrap_the_linker() {
    let temp = project();
    let root = temp.path();
    let (stdout, stderr) = ok(mitts(root, &["check", "-vv"]));
    assert!(!stdout.contains("linker: "), "{stdout}");
    assert!(!stderr.contains("MITTS_LINK_TIMING"), "{stderr}");
}

/// A library unit (an rlib) does not link; only the binary gets the wrapper.
#[test]
fn only_linking_units_get_the_wrapper() {
    let temp = project();
    let root = temp.path();
    let (_, stderr) = ok(mitts(root, &["build", "-vv"]));
    let helper = stderr
        .lines()
        .find(|l| l.contains("Running `") && l.contains("--crate-name helper"))
        .expect("helper command");
    assert!(!helper.contains("MITTS_LINK_TIMING"), "{helper}");
    let app = stderr
        .lines()
        .find(|l| {
            l.contains("Running `")
                && l.contains("--crate-name app ")
                && !l.contains("build_script")
        })
        .expect("app command");
    if cfg!(unix) {
        assert!(app.contains("MITTS_LINK_TIMING_LINKER=cc"), "{app}");
    }
}

/// A second build in the same project reports only its own rustc time: the summary of an
/// all-cached build has no rustc part at all.
#[test]
fn an_all_cached_build_reports_no_compile_time() {
    let temp = project();
    let root = temp.path();
    ok(mitts(root, &["build", "-v"]));
    let (stdout, _) = ok(mitts(root, &["build", "-v"]));
    let time = stdout
        .lines()
        .find(|l| l.contains("time: "))
        .expect("time line");
    assert!(!time.contains("rustc "), "{time}");
    assert!(!time.contains("link "), "{time}");
    assert!(time.contains("cache restore "), "{time}");
}

/// `RUSTC` wins over `+toolchain`, as it does for Cargo, and the summary says so.
#[test]
fn rustc_wins_over_plus_toolchain_and_is_named() {
    let Some(toolchain) = active_rustup_toolchain() else {
        eprintln!("rustup is not installed; skipping");
        return;
    };
    let rustc = Command::new("rustup")
        .args(["which", "rustc", "--toolchain", &toolchain])
        .output()
        .unwrap();
    let rustc = String::from_utf8_lossy(&rustc.stdout).trim().to_string();
    let temp = project();
    let plus = format!("+{toolchain}");
    let (stdout, _) = ok(mitts_with(
        temp.path(),
        &[plus.as_str(), "build", "-v"],
        &[("RUSTC", rustc.as_str())],
    ));
    assert!(stdout.contains("(selected by RUSTC)"), "{stdout}");
}

fn active_rustup_toolchain() -> Option<String> {
    let output = Command::new("rustup")
        .args(["show", "active-toolchain"])
        .env_remove("RUSTUP_TOOLCHAIN")
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    text.split_whitespace().next().map(str::to_string)
}

#[test]
fn plus_toolchain_selects_a_rustup_toolchain() {
    let Some(toolchain) = active_rustup_toolchain() else {
        eprintln!("rustup is not installed; skipping");
        return;
    };
    let temp = project();
    let root = temp.path();
    let plus = format!("+{toolchain}");
    let (stdout, _) = ok(mitts_with(
        root,
        &[plus.as_str(), "build", "-v"],
        &[("RUSTC", "")],
    ));
    assert!(
        stdout.contains(&format!("selected by +{toolchain} on the command line")),
        "{stdout}"
    );
    assert!(stdout.contains(&format!("· {toolchain}")), "{stdout}");
}

/// With a nightly toolchain installed (the OS matrix installs one), `+nightly` builds
/// with it, says so, and does not reuse the stable build's cache entries.
#[test]
fn plus_nightly_builds_with_the_nightly_compiler() {
    let installed = Command::new("rustup")
        .args(["which", "rustc", "--toolchain", "nightly"])
        .env_remove("RUSTUP_TOOLCHAIN")
        .output()
        .is_ok_and(|o| o.status.success());
    if !installed {
        eprintln!("no nightly toolchain; skipping");
        return;
    }
    let temp = project();
    let root = temp.path();
    ok(mitts_with(root, &["build"], &[("RUSTC", "")]));
    let (stdout, _) = ok(mitts_with(
        root,
        &["+nightly", "build", "-v"],
        &[("RUSTC", "")],
    ));
    assert!(stdout.contains("-nightly"), "{stdout}");
    assert!(
        stdout.contains("selected by +nightly on the command line"),
        "{stdout}"
    );
    assert!(stdout.contains("2 compiled"), "{stdout}");
    let exe = root
        .join("target/debug")
        .join(format!("app{}", std::env::consts::EXE_SUFFIX));
    assert!(Command::new(exe).status().unwrap().success());
}

#[test]
fn plus_toolchain_rejects_unknown_toolchains_and_a_bare_plus() {
    let temp = project();
    let root = temp.path();
    let unknown = mitts(root, &["+no-such-toolchain-for-mitts", "build"]);
    assert!(!unknown.status.success());
    let stderr = String::from_utf8_lossy(&unknown.stderr);
    assert!(stderr.contains("no-such-toolchain-for-mitts"), "{stderr}");
    let bare = mitts(root, &["+", "build"]);
    assert!(!bare.status.success());
    assert!(String::from_utf8_lossy(&bare.stderr).contains("toolchain name"));
}
