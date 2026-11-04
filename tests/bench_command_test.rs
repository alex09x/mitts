/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! `mitts bench` compiles the benchmark targets in the bench profile and runs each with
//! `--bench` (issue 43). The library has a unit test and a documentation test that fail when
//! they run, so a pass shows that neither is part of a benchmark run.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use tempfile::TempDir;

fn write(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

const BENCH_MAIN: &str = "fn main() {\n    let args: Vec<String> = std::env::args().skip(1).collect();\n    \
     println!(\"BENCH-RAN {} args={:?} debug_assertions={}\", env!(\"CARGO_CRATE_NAME\"), args, cfg!(debug_assertions));\n}\n";

fn project() -> TempDir {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    write(
        &root.join("Cargo.toml"),
        "[package]\nname = \"benchy\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
         [[bench]]\nname = \"speed\"\nharness = false\n\n\
         [[bench]]\nname = \"other\"\nharness = false\n",
    );
    write(
        &root.join("src/lib.rs"),
        "/// ```\n/// panic!(\"a documentation test must not run in a benchmark run\");\n/// ```\n\
         pub fn f() -> u32 { 1 }\n\n\
         #[cfg(test)]\nmod tests {\n    #[test]\n    fn unit() { panic!(\"a unit test must not run in a benchmark run\"); }\n}\n",
    );
    write(&root.join("benches/speed.rs"), BENCH_MAIN);
    write(&root.join("benches/other.rs"), BENCH_MAIN);
    dir
}

fn mitts(dir: &TempDir, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mitts"))
        .current_dir(dir.path())
        .arg("bench")
        .arg("--offline")
        .arg("--cache-dir")
        .arg(dir.path().join("cache"))
        .args(args)
        .env("CARGO_HOME", dir.path().join("cargo-home"))
        .output()
        .unwrap()
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn ran(output: &Output) -> Vec<String> {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| line.starts_with("BENCH-RAN "))
        .map(str::to_owned)
        .collect()
}

#[test]
fn runs_every_benchmark_target_with_the_bench_argument_in_the_release_profile() {
    let dir = project();
    let output = mitts(&dir, &[]);
    assert!(output.status.success(), "{}", text(&output));
    let lines = ran(&output);
    assert_eq!(lines.len(), 2, "{}", text(&output));
    for name in ["speed", "other"] {
        let line = lines
            .iter()
            .find(|l| l.starts_with(&format!("BENCH-RAN {name} ")))
            .unwrap_or_else(|| panic!("{name} did not run:\n{}", text(&output)));
        assert!(line.contains("args=[\"--bench\"]"), "{line}");
        assert!(line.contains("debug_assertions=false"), "{line}");
    }
}

#[test]
fn bench_names_select_targets_and_the_filter_and_trailing_arguments_follow() {
    let dir = project();
    let output = mitts(
        &dir,
        &["--bench", "speed", "fast", "--", "--save-baseline", "main"],
    );
    assert!(output.status.success(), "{}", text(&output));
    let lines = ran(&output);
    assert_eq!(lines.len(), 1, "{}", text(&output));
    assert!(
        lines[0].starts_with("BENCH-RAN speed ")
            && lines[0].contains("args=[\"fast\", \"--bench\", \"--save-baseline\", \"main\"]"),
        "{}",
        lines[0]
    );
}

#[test]
fn no_run_compiles_the_benchmarks_without_running_them() {
    let dir = project();
    let output = mitts(&dir, &["--no-run"]);
    assert!(output.status.success(), "{}", text(&output));
    assert!(ran(&output).is_empty(), "{}", text(&output));
    let built = fs::read_dir(dir.path().join("target/release/deps"))
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("benchy-speed"))
        .count();
    assert!(built > 0, "the speed benchmark is compiled");
}

#[test]
fn a_package_without_benchmarks_says_so_and_succeeds() {
    let dir = TempDir::new().unwrap();
    write(
        &dir.path().join("Cargo.toml"),
        "[package]\nname = \"plain\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write(&dir.path().join("src/lib.rs"), "pub fn f() {}\n");
    let output = mitts(&dir, &[]);
    assert!(output.status.success(), "{}", text(&output));
    assert!(
        text(&output).contains("no benchmark targets"),
        "{}",
        text(&output)
    );
}
