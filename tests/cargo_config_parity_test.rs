/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! `.cargo/config.toml` keys that Mitts ignored until issue 109 showed it (Melodia's
//! `[build] jobs = 8` had no effect). Each test sets one key, through the file, the
//! `CARGO_*` variable or `--config`, and runs the real binary on a fresh workspace with
//! its own cache.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Instant;
use tempfile::TempDir;

struct Ws {
    dir: TempDir,
}

impl Ws {
    /// A package `app` with one binary that prints `hello`, and an empty `.cargo/config.toml`.
    fn new() -> Self {
        let ws = Self {
            dir: TempDir::new().unwrap(),
        };
        ws.write(
            "Cargo.toml",
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        );
        ws.write("src/main.rs", "fn main() { println!(\"hello\"); }\n");
        ws.write(".cargo/config.toml", "");
        ws
    }

    fn root(&self) -> &Path {
        self.dir.path()
    }

    fn write(&self, rel: &str, text: &str) {
        let path = self.root().join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn config(&self, text: &str) {
        self.write(".cargo/config.toml", text);
    }

    /// An executable shell script that appends its arguments to `log` and runs `exec`.
    fn script(&self, rel: &str, log: &str, exec: &str) -> PathBuf {
        let path = self.root().join(rel);
        let log = self.root().join(log);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\n{exec}\n",
                log.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn read(&self, rel: &str) -> String {
        fs::read_to_string(self.root().join(rel)).unwrap_or_default()
    }

    fn mitts(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mitts"));
        cmd.current_dir(self.root())
            .args(args)
            .env("MITTS_CACHE_DIR", self.root().join("cache"))
            .env("CARGO_HOME", self.root().join("cargo-home"))
            .env("MITTS_NET_RETRY", "0");
        for var in [
            "RUSTC",
            "RUSTDOC",
            "RUSTC_WRAPPER",
            "RUSTC_WORKSPACE_WRAPPER",
            "RUSTFLAGS",
            "RUSTDOCFLAGS",
            "CARGO_ENCODED_RUSTFLAGS",
            "CARGO_ENCODED_RUSTDOCFLAGS",
            "CARGO_INCREMENTAL",
            "CARGO_BUILD_JOBS",
            "MITTS_JOBS",
            "CARGO_TERM_VERBOSE",
            "CARGO_TERM_QUIET",
            "CARGO_NET_OFFLINE",
            "HTTPS_PROXY",
            "https_proxy",
            "HTTP_PROXY",
            "http_proxy",
            "ALL_PROXY",
            "all_proxy",
        ] {
            cmd.env_remove(var);
        }
        cmd
    }
}

fn run(cmd: &mut Command) -> Output {
    cmd.output().unwrap()
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn ok(out: &Output) -> String {
    let all = text(out);
    assert!(out.status.success(), "command failed:\n{all}");
    all
}

fn host() -> String {
    let out = Command::new("rustc").arg("-vV").output().unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find_map(|l| l.strip_prefix("host: "))
        .unwrap()
        .to_string()
}

fn env_triple(triple: &str) -> String {
    triple.replace('-', "_").to_ascii_uppercase()
}

/// Four packages whose build scripts each sleep one second.
fn sleeping_workspace() -> Ws {
    let ws = Ws::new();
    ws.write(
        "Cargo.toml",
        "[workspace]\nmembers = [\"a\", \"b\", \"c\", \"d\"]\nresolver = \"2\"\n",
    );
    fs::remove_dir_all(ws.root().join("src")).unwrap();
    for name in ["a", "b", "c", "d"] {
        ws.write(
            &format!("{name}/Cargo.toml"),
            &format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
        );
        ws.write(&format!("{name}/src/lib.rs"), "");
        ws.write(
            &format!("{name}/build.rs"),
            "fn main() { std::thread::sleep(std::time::Duration::from_secs(1)); }\n",
        );
    }
    ws
}

fn timed(cmd: &mut Command) -> f64 {
    let start = Instant::now();
    ok(&run(cmd));
    start.elapsed().as_secs_f64()
}

#[test]
fn build_jobs_limits_parallel_units_from_the_file_the_env_and_the_command_line() {
    let ws = sleeping_workspace();
    ws.config("[build]\njobs = 1\n");
    let t = timed(&mut ws.mitts(&["build"]));
    assert!(
        t >= 3.8,
        "build.jobs = 1 ran the four 1 s scripts in {t:.1}s"
    );

    let ws = sleeping_workspace();
    let t = timed(ws.mitts(&["build"]).env("CARGO_BUILD_JOBS", "1"));
    assert!(t >= 3.8, "CARGO_BUILD_JOBS=1: {t:.1}s");

    let ws = sleeping_workspace();
    let t = timed(&mut ws.mitts(&["build", "--config", "build.jobs=1"]));
    assert!(t >= 3.8, "--config build.jobs=1: {t:.1}s");

    let ws = sleeping_workspace();
    ws.config("[build]\njobs = 0\n");
    let out = run(&mut ws.mitts(&["build"]));
    assert!(
        !out.status.success() && text(&out).contains("may not be 0"),
        "{}",
        text(&out)
    );
}

#[test]
fn build_rustc_and_build_rustdoc_choose_the_programs() {
    let ws = Ws::new();
    let rustc = ws.script("tools/rustc", "rustc.log", "exec rustc \"$@\"");
    ws.config(&format!("[build]\nrustc = \"{}\"\n", rustc.display()));
    ok(&run(&mut ws.mitts(&["build"])));
    assert!(
        ws.read("rustc.log").contains("--crate-name app"),
        "{}",
        ws.read("rustc.log")
    );

    // A relative program is relative to the directory holding `.cargo`.
    let ws = Ws::new();
    ws.script("tools/rustc", "rustc.log", "exec rustc \"$@\"");
    ok(&run(ws
        .mitts(&["build"])
        .env("CARGO_BUILD_RUSTC", ws.root().join("tools/rustc"))));
    assert!(ws.read("rustc.log").contains("--crate-name app"));

    let ws = Ws::new();
    ws.script("tools/rustc", "rustc.log", "exec rustc \"$@\"");
    ws.config("[build]\nrustc = \"tools/rustc\"\n");
    ok(&run(&mut ws.mitts(&["build"])));
    assert!(ws.read("rustc.log").contains("--crate-name app"));

    let ws = Ws::new();
    ws.write(
        "src/lib.rs",
        "/// ```\n/// assert!(cfg!(probe_rdf));\n/// ```\npub fn f() {}\n",
    );
    ws.script("tools/rustdoc", "rustdoc.log", "exec rustdoc \"$@\"");
    ws.config("[build]\nrustdoc = \"tools/rustdoc\"\nrustdocflags = [\"--cfg\", \"probe_rdf\"]\n");
    ok(&run(&mut ws.mitts(&["test", "--doc"])));
    let log = ws.read("rustdoc.log");
    assert!(log.contains("--test") && log.contains("probe_rdf"), "{log}");
}

#[test]
fn build_incremental_false_turns_incremental_off() {
    let ws = Ws::new();
    ws.config("[build]\nincremental = false\n");
    let all = ok(&run(&mut ws.mitts(&["build", "-v"])));
    assert!(all.contains("--crate-name app"), "{all}");
    assert!(!all.contains("incremental="), "{all}");

    let ws = Ws::new();
    let all = ok(&run(ws
        .mitts(&["build", "-v"])
        .env("CARGO_BUILD_INCREMENTAL", "false")));
    assert!(!all.contains("incremental="), "{all}");
}

#[test]
fn aliases_expand_with_cargo_precedence_and_never_loop() {
    let ws = Ws::new();
    ws.config("[alias]\nbb = \"build --release\"\nxx = [\"bb\"]\n");
    ok(&run(&mut ws.mitts(&["xx"])));
    assert!(ws.root().join("target/release/app").is_file());

    // --config beats CARGO_ALIAS_* beats the file.
    let ws = Ws::new();
    ws.config("[alias]\nmk = \"check\"\n");
    ok(&run(ws
        .mitts(&["mk"])
        .env("CARGO_ALIAS_MK", "build --release")));
    assert!(ws.root().join("target/release/app").is_file());
    let ws = Ws::new();
    ok(&run(ws
        .mitts(&["mk", "--config", "alias.mk=\"check\""])
        .env("CARGO_ALIAS_MK", "build --release")));
    assert!(!ws.root().join("target/release/app").exists());

    let ws = Ws::new();
    ok(&run(&mut ws.mitts(&["b"])));
    assert!(ws.root().join("target/debug/app").is_file());

    let ws = Ws::new();
    ws.config("[alias]\nx = \"y\"\ny = \"x\"\n");
    let out = run(&mut ws.mitts(&["x"]));
    assert!(
        !out.status.success() && text(&out).contains("recursive"),
        "{}",
        text(&out)
    );

    // A user alias never shadows a command.
    let ws = Ws::new();
    ws.config("[alias]\nbuild = \"check\"\n");
    ok(&run(&mut ws.mitts(&["build"])));
    assert!(ws.root().join("target/debug/app").is_file());
}

#[test]
fn runners_start_run_and_test_executables() {
    let host = host();
    let ws = Ws::new();
    ws.write(
        "src/main.rs",
        "fn main() { println!(\"hello\"); }\n#[test]\nfn t() {}\n",
    );
    ws.script(
        "tools/emu",
        "runner.log",
        "[ \"$1\" = --fake-flag ] && shift\nexec \"$@\"",
    );
    ws.config(&format!(
        "[target.{host}]\nrunner = [\"tools/emu\", \"--fake-flag\"]\n"
    ));
    let all = ok(&run(&mut ws.mitts(&["run"])));
    assert!(all.contains("hello"), "{all}");
    let log = ws.read("runner.log");
    assert!(log.contains("--fake-flag") && log.contains("app"), "{log}");

    fs::remove_file(ws.root().join("runner.log")).unwrap();
    ok(&run(&mut ws.mitts(&["test"])));
    assert!(
        !ws.read("runner.log").is_empty(),
        "test did not use the runner"
    );

    fs::remove_file(ws.root().join("runner.log")).unwrap();
    ok(&run(&mut ws.mitts(&["test", "-c=2"])));
    assert!(
        !ws.read("runner.log").is_empty(),
        "test -c did not use the runner"
    );

    let ws = Ws::new();
    ws.script("tools/emu", "runner.log", "exec \"$@\"");
    ws.config("[target.'cfg(unix)']\nrunner = \"tools/emu\"\n");
    ok(&run(&mut ws.mitts(&["run"])));
    assert!(!ws.read("runner.log").is_empty(), "cfg(unix) runner unused");

    let ws = Ws::new();
    let emu = ws.script("tools/emu", "runner.log", "exec \"$@\"");
    ok(&run(ws.mitts(&["run"]).env(
        format!("CARGO_TARGET_{}_RUNNER", env_triple(&host)),
        &emu,
    )));
    assert!(
        !ws.read("runner.log").is_empty(),
        "CARGO_TARGET_<T>_RUNNER unused"
    );
}

const CFG_BIN: &str =
    "fn main() { if cfg!(probe_cfg) { println!(\"CFG_ON\") } else { println!(\"CFG_OFF\") } }\n";

#[test]
fn target_rustflags_from_the_environment_reach_rustc() {
    let ws = Ws::new();
    ws.write("src/main.rs", CFG_BIN);
    let all = ok(&run(ws.mitts(&["run"]).env(
        format!("CARGO_TARGET_{}_RUSTFLAGS", env_triple(&host())),
        "--cfg probe_cfg",
    )));
    assert!(all.contains("CFG_ON"), "{all}");
}

#[test]
fn a_links_override_replaces_the_build_script() {
    let ws = Ws::new();
    ws.write(
        "Cargo.toml",
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\nlinks = \"probe\"\nbuild = \"build.rs\"\n",
    );
    ws.write(
        "build.rs",
        "fn main() { panic!(\"the override must replace this script\"); }\n",
    );
    ws.write(
        "src/main.rs",
        "fn main() { println!(\"{} {}\", if cfg!(probe_cfg) { \"CFG_ON\" } else { \"CFG_OFF\" }, env!(\"PROBE_ENV\")); }\n",
    );
    ws.config(&format!(
        "[target.{}.probe]\nrustc-cfg = [\"probe_cfg\"]\nrustc-env = {{ PROBE_ENV = \"from_override\" }}\n",
        host()
    ));
    let all = ok(&run(&mut ws.mitts(&["run"])));
    assert!(all.contains("CFG_ON from_override"), "{all}");
    assert!(!all.contains("must replace"), "{all}");
}

fn with_registry_dependency() -> Ws {
    let ws = Ws::new();
    ws.write(
        "Cargo.toml",
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nitoa = \"=1.0.11\"\n",
    );
    ws
}

#[test]
fn net_offline_never_goes_to_the_network() {
    let ws = with_registry_dependency();
    ws.config("[net]\noffline = true\n");
    let out = run(&mut ws.mitts(&["build"]));
    assert!(!out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("offline"), "{}", text(&out));

    let ws = with_registry_dependency();
    let out = run(ws.mitts(&["fetch"]).env("CARGO_NET_OFFLINE", "true"));
    assert!(!out.status.success(), "{}", text(&out));
}

#[test]
fn http_proxy_cainfo_and_timeout_are_honoured() {
    // A dead proxy: when it is used, nothing can be fetched.
    let ws = with_registry_dependency();
    ws.config("[http]\nproxy = \"http://127.0.0.1:9\"\n");
    let out = run(&mut ws.mitts(&["fetch"]));
    assert!(
        !out.status.success(),
        "http.proxy was not used:\n{}",
        text(&out)
    );

    let ws = with_registry_dependency();
    let out = run(ws
        .mitts(&["fetch"])
        .env("HTTPS_PROXY", "http://127.0.0.1:9"));
    assert!(
        !out.status.success(),
        "HTTPS_PROXY was not used:\n{}",
        text(&out)
    );

    let ws = with_registry_dependency();
    ws.write("bad.pem", "not a certificate\n");
    ws.config("[http]\ncainfo = \"bad.pem\"\n");
    let out = run(&mut ws.mitts(&["fetch"]));
    assert!(
        !out.status.success() && text(&out).contains("http.cainfo"),
        "{}",
        text(&out)
    );

    let ws = Ws::new();
    ws.config("[http]\ntimeout = 0\n");
    let out = run(&mut ws.mitts(&["build"]));
    assert!(
        !out.status.success() && text(&out).contains("http.timeout"),
        "{}",
        text(&out)
    );
}

#[test]
fn term_verbose_and_quiet_act_like_the_flags() {
    let ws = Ws::new();
    ws.config("[term]\nverbose = true\n");
    let all = ok(&run(&mut ws.mitts(&["build"])));
    assert!(all.contains("--crate-name app"), "{all}");

    let ws = Ws::new();
    ws.config("[term]\nquiet = true\n");
    let all = ok(&run(&mut ws.mitts(&["build"])));
    assert!(!all.contains("Mitts Build"), "{all}");
    // The command line wins over the configuration.
    let ws = Ws::new();
    ws.config("[term]\nquiet = true\n");
    let all = ok(&run(&mut ws.mitts(&["build", "-v"])));
    assert!(all.contains("--crate-name app"), "{all}");

    let ws = Ws::new();
    ws.config("[term]\nquiet = true\nverbose = true\n");
    let out = run(&mut ws.mitts(&["build"]));
    assert!(
        !out.status.success() && text(&out).contains("cannot set both"),
        "{}",
        text(&out)
    );
}
