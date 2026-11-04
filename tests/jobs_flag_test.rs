/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! `-j/--jobs N` caps concurrent native build units like `MITTS_JOBS`, and wins over it.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Two build scripts that record how many of them ran at once and the `NUM_JOBS` they saw.
fn project(root: &Path, state: &Path) {
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(state).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname='root'\nversion='0.1.0'\nedition='2021'\n\
         [dependencies]\na={path='a'}\nb={path='b'}\n",
    )
    .unwrap();
    std::fs::write(root.join("src/lib.rs"), "pub fn f() { a::f(); b::f(); }\n").unwrap();
    let script = r#"
        fn main() {
            println!("cargo:rerun-if-env-changed=SHARED_STATE");
            let state = std::path::PathBuf::from(std::env::var_os("SHARED_STATE").unwrap());
            let lock = state.join("lock");
            let acquire = || { while std::fs::create_dir(&lock).is_err() { std::thread::sleep(std::time::Duration::from_millis(2)); } };
            acquire();
            let running: usize = std::fs::read_to_string(state.join("running")).unwrap_or_default().parse().unwrap_or(0);
            let maximum: usize = std::fs::read_to_string(state.join("max")).unwrap_or_default().parse().unwrap_or(0);
            std::fs::write(state.join("running"), (running + 1).to_string()).unwrap();
            std::fs::write(state.join("max"), maximum.max(running + 1).to_string()).unwrap();
            std::fs::write(state.join(format!("jobs-{}", std::env::var("CARGO_PKG_NAME").unwrap())), std::env::var("NUM_JOBS").unwrap()).unwrap();
            std::fs::remove_dir(&lock).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(150));
            acquire();
            let running: usize = std::fs::read_to_string(state.join("running")).unwrap().parse().unwrap();
            std::fs::write(state.join("running"), (running - 1).to_string()).unwrap();
            std::fs::remove_dir(&lock).unwrap();
        }
    "#;
    for name in ["a", "b"] {
        let dir = root.join(name);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(
            dir.join("Cargo.toml"),
            format!("[package]\nname='{name}'\nversion='0.1.0'\nedition='2021'\n"),
        )
        .unwrap();
        std::fs::write(dir.join("src/lib.rs"), "pub fn f() {}\n").unwrap();
        std::fs::write(dir.join("build.rs"), script).unwrap();
    }
}

struct Fixture {
    temp: tempfile::TempDir,
    root: PathBuf,
    state: PathBuf,
}

fn fixture() -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    let state = temp.path().join("state");
    project(&root, &state);
    Fixture { temp, root, state }
}

impl Fixture {
    /// One build with a fresh cache and target, so the build scripts really run.
    fn run(&self, flags: &[&str], jobs_env: Option<&str>) -> Output {
        let _ = std::fs::remove_dir_all(&self.state);
        std::fs::create_dir_all(&self.state).unwrap();
        let unique = self.temp.path().join(format!("cache-{}", flags.join("_")));
        let mut command = Command::new(env!("CARGO_BIN_EXE_mitts"));
        command
            .current_dir(&self.root)
            .args(["build", "--offline", "--cache-dir"])
            .arg(&unique)
            .arg("--target-dir")
            .arg(self.temp.path().join(format!("target-{}", flags.join("_"))))
            .args(flags)
            .env("MITTS_FAST_LOCK", "1")
            .env("SHARED_STATE", &self.state)
            .env_remove("MITTS_JOBS")
            .env_remove("CARGO_MAKEFLAGS")
            .env_remove("MAKEFLAGS");
        if let Some(value) = jobs_env {
            command.env("MITTS_JOBS", value);
        }
        command.output().unwrap()
    }

    fn max_concurrency(&self) -> usize {
        std::fs::read_to_string(self.state.join("max"))
            .unwrap()
            .parse()
            .unwrap()
    }

    fn num_jobs(&self, package: &str) -> String {
        std::fs::read_to_string(self.state.join(format!("jobs-{package}"))).unwrap()
    }
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
}

#[test]
fn jobs_flag_limits_concurrent_units_and_reaches_build_scripts() {
    let fixture = fixture();
    let output = fixture.run(&["-j", "1"], None);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(fixture.max_concurrency(), 1);
    assert_eq!(fixture.num_jobs("a"), "1");
    assert_eq!(fixture.num_jobs("b"), "1");
}

#[test]
fn long_flag_and_equals_spelling_are_accepted() {
    let fixture = fixture();
    for flags in [&["--jobs", "1"][..], &["--jobs=1"][..]] {
        let output = fixture.run(flags, None);
        assert!(output.status.success(), "{flags:?}: {}", stderr(&output));
        assert_eq!(fixture.num_jobs("a"), "1", "{flags:?}");
    }
}

#[test]
fn flag_wins_over_the_environment_variable_in_both_directions() {
    let fixture = fixture();
    let output = fixture.run(&["-j", "2"], Some("1"));
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(fixture.num_jobs("a"), "2", "flag 2 beats MITTS_JOBS=1");

    let output = fixture.run(&["-j", "1"], Some("4"));
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(fixture.num_jobs("a"), "1", "flag 1 beats MITTS_JOBS=4");
    assert_eq!(fixture.max_concurrency(), 1);
}

#[test]
fn environment_variable_still_applies_without_the_flag() {
    let fixture = fixture();
    let output = fixture.run(&[], Some("1"));
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(fixture.num_jobs("a"), "1");
}

#[test]
fn zero_and_non_numeric_values_are_usage_errors() {
    let fixture = fixture();
    for bad in ["0", "many"] {
        let output = fixture.run(&["-j", bad], None);
        assert_eq!(
            output.status.code(),
            Some(2),
            "-j {bad}: {}",
            stderr(&output)
        );
        assert!(
            stderr(&output).contains("--jobs") || stderr(&output).contains("-j"),
            "-j {bad}: {}",
            stderr(&output)
        );
    }
}

#[test]
fn check_and_test_commands_take_the_flag() {
    let fixture = fixture();
    for command in ["check", "test"] {
        let output = Command::new(env!("CARGO_BIN_EXE_mitts"))
            .current_dir(&fixture.root)
            .args([command, "--help"])
            .output()
            .unwrap();
        let help = String::from_utf8_lossy(&output.stdout).to_string();
        assert!(
            help.contains("--jobs"),
            "{command} help lists --jobs: {help}"
        );
        assert!(
            help.contains("MITTS_JOBS"),
            "{command} help names the variable"
        );
    }
}
