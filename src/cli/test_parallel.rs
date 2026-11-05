/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! `mitts test -c` / `--concurrency[=N]`: every test in a process of its own, many at a time.
//!
//! The way cargo-nextest does it: each test binary lists its tests (`--list`), then every test
//! runs as `binary --exact NAME` in a separate process, `N` processes at once. A test cannot
//! affect another through shared state, a crash or a hang loses one test, and the tests of all
//! binaries share one pool instead of running binary after binary. Without a number the pool
//! is two thirds of the cores. Output of a test is captured and shown when it fails.
//!
//! A binary without the standard harness (`harness = false`) is run once as a whole. Arguments
//! after `--` are not passed to the test processes in this mode.

use super::test::TestArgs;
use super::test_runner::DiscoveredBinaries;
use crate::cli::ui::Ui;
use crate::core::action::BuildPlan;
use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Instant;

/// The number of processes for `--concurrency[=N]`: `N`, or two thirds of the cores (at least
/// one) when no number was given (0).
pub fn jobs_for(requested: usize) -> usize {
    if requested > 0 {
        return requested;
    }
    let cores = std::thread::available_parallelism().map_or(2, |n| n.get());
    two_thirds(cores)
}

fn two_thirds(cores: usize) -> usize {
    (cores * 2 / 3).max(1)
}

/// The test names of a libtest `--list` output.
pub fn parse_list(output: &str) -> BTreeSet<String> {
    output
        .lines()
        .filter_map(|line| line.strip_suffix(": test"))
        .map(str::to_owned)
        .collect()
}

/// Whether `name` is selected by the filter of `mitts test [TESTNAME]` (and `--exact`).
pub fn selected(name: &str, filter: Option<&str>, exact: bool) -> bool {
    match filter {
        None => true,
        Some(f) if exact => name == f,
        Some(f) => name.contains(f),
    }
}

struct Target {
    label: String,
    path: PathBuf,
    env: HashMap<String, String>,
    work_dir: PathBuf,
    /// No standard harness: run once, do not list.
    whole: bool,
}

/// The tests a binary lists, and the ones among them that are ignored.
type Listing = (BTreeSet<String>, BTreeSet<String>);

struct Job {
    target: usize,
    /// `None` runs the whole binary.
    name: Option<String>,
    ignored: bool,
}

struct Context<'a> {
    binaries: &'a DiscoveredBinaries,
    plan: &'a BuildPlan,
    release: bool,
    targets: Vec<Target>,
    /// Added to every test process (for example the profile file pattern of a coverage run).
    extra_env: Vec<(String, std::ffi::OsString)>,
}

impl Context<'_> {
    fn command(&self, target: &Target) -> Command {
        let mut cmd = Command::new(&target.path);
        cmd.current_dir(&target.work_dir);
        crate::adapter::rust::unit_env::apply_runtime_execution_env(
            &mut cmd,
            &target.work_dir,
            &target.env,
            self.plan,
            self.release,
        );
        if !target.env.contains_key("CARGO_PKG_NAME") {
            for (name, file) in &self.binaries.cli_binaries {
                cmd.env(format!("CARGO_BIN_EXE_{name}"), file);
            }
        }
        for (key, value) in &self.extra_env {
            cmd.env(key, value);
        }
        cmd
    }

    fn list(&self, target: &Target, ignored_only: bool) -> Option<BTreeSet<String>> {
        let mut cmd = self.command(target);
        cmd.arg("--list");
        if ignored_only {
            cmd.arg("--ignored");
        }
        let output = cmd.output().ok()?;
        output
            .status
            .success()
            .then(|| parse_list(&String::from_utf8_lossy(&output.stdout)))
    }
}

/// Runs `work(index, item)` for every item on `jobs` threads.
fn parallel_for<T: Sync>(items: &[T], jobs: usize, work: impl Fn(usize, &T) + Sync) {
    let next = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        for _ in 0..jobs.clamp(1, items.len().max(1)) {
            scope.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                if i >= items.len() {
                    break;
                }
                work(i, &items[i]);
            });
        }
    });
}

#[allow(clippy::too_many_arguments)]
pub fn run(
    binaries: &DiscoveredBinaries,
    args: &TestArgs,
    root_dir: &std::path::Path,
    plan: &BuildPlan,
    profile: &str,
    start: Instant,
    requested_jobs: usize,
    extra_env: &[(String, std::ffi::OsString)],
) -> i32 {
    let jobs = jobs_for(requested_jobs);
    let targets: Vec<Target> = binaries
        .test_binaries
        .iter()
        .map(|(description, path, env)| {
            let manifest_dir = env
                .get("CARGO_MANIFEST_DIR")
                .filter(|d| !d.is_empty() && d.as_str() != ".");
            let work_dir = match manifest_dir {
                Some(dir) if std::path::Path::new(dir).is_absolute() => PathBuf::from(dir),
                Some(dir) => root_dir.join(dir),
                None => root_dir.to_path_buf(),
            };
            Target {
                label: path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| description.clone()),
                path: if path.is_absolute() {
                    path.clone()
                } else {
                    root_dir.join(path)
                },
                env: env.clone(),
                work_dir,
                whole: description.contains("::custom-test:"),
            }
        })
        .collect();
    let context = Context {
        binaries,
        plan,
        release: args.release,
        targets,
        extra_env: extra_env.to_vec(),
    };

    // List the tests of all binaries at once.
    let listed: Mutex<Vec<Option<Listing>>> = Mutex::new(vec![None; context.targets.len()]);
    parallel_for(&context.targets, jobs, |i, target| {
        if target.whole {
            return;
        }
        if let (Some(all), Some(ignored)) =
            (context.list(target, false), context.list(target, true))
        {
            listed.lock().unwrap()[i] = Some((all, ignored));
        }
    });
    let listed = listed.into_inner().unwrap();

    let mut queue: Vec<Job> = Vec::new();
    let mut skipped = 0usize;
    for (i, listing) in listed.iter().enumerate() {
        match listing {
            Some((all, ignored)) => {
                for name in all {
                    if !selected(name, args.filter.as_deref(), args.exact) {
                        continue;
                    }
                    let is_ignored = ignored.contains(name);
                    if is_ignored != args.ignored {
                        skipped += 1;
                        continue;
                    }
                    queue.push(Job {
                        target: i,
                        name: Some(name.clone()),
                        ignored: is_ignored,
                    });
                }
            }
            // A harness of its own, or a binary that did not list: run it as a whole.
            None => queue.push(Job {
                target: i,
                name: None,
                ignored: false,
            }),
        }
    }
    if queue.is_empty() {
        println!("mitts test: no tests matched ({skipped} skipped)");
        return 0;
    }

    Ui::stage_start(
        "🧪",
        &format!("Running {} tests in {jobs} processes", queue.len()),
    );
    let stop = AtomicBool::new(false);
    let print = Mutex::new(());
    let outcome = Mutex::new((0usize, Vec::<String>::new()));
    let run_started = Instant::now();
    parallel_for(&queue, jobs, |_, job| {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        let target = &context.targets[job.target];
        let mut cmd = context.command(target);
        let shown = match &job.name {
            Some(name) => {
                cmd.args(["--exact", name, "--nocapture"]);
                if job.ignored {
                    cmd.arg("--ignored");
                }
                format!("{} {name}", target.label)
            }
            None => {
                if let Some(filter) = &args.filter {
                    cmd.arg(filter);
                }
                target.label.clone()
            }
        };
        let started = Instant::now();
        let result = cmd.output();
        let seconds = started.elapsed().as_secs_f64();
        let (ok, captured) = match result {
            Ok(output) => (
                output.status.success(),
                format!(
                    "{}{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                ),
            ),
            Err(err) => (
                false,
                format!("failed to start {}: {err}", target.path.display()),
            ),
        };
        let _guard = print.lock().unwrap();
        if ok {
            println!("        PASS [{seconds:8.3}s] {shown}");
            outcome.lock().unwrap().0 += 1;
        } else {
            println!("        FAIL [{seconds:8.3}s] {shown}");
            for line in captured.trim_end().lines() {
                println!("             {line}");
            }
            outcome.lock().unwrap().1.push(shown);
            if !args.no_fail_fast {
                stop.store(true, Ordering::Relaxed);
            }
        }
    });

    let (passed, failed) = outcome.into_inner().unwrap();
    let not_run = queue.len() - passed - failed.len();
    let elapsed = format!("{:.2}s", start.elapsed().as_secs_f64());
    println!(
        "     Summary [{:8.3}s] {} tests run: {passed} passed, {} failed, {skipped} skipped{}",
        run_started.elapsed().as_secs_f64(),
        passed + failed.len(),
        failed.len(),
        if not_run > 0 {
            format!(", {not_run} not run after a failure")
        } else {
            String::new()
        }
    );
    if failed.is_empty() {
        Ui::finish(profile, &elapsed, &format!("{passed} tests passed"));
        0
    } else {
        for name in &failed {
            println!("        FAIL {name}");
        }
        Ui::finish_failed(profile, &elapsed, &format!("{} tests failed", failed.len()));
        101
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_thirds_of_the_cores_by_default_and_the_requested_number_otherwise() {
        assert!(jobs_for(0) >= 1);
        assert_eq!(jobs_for(7), 7);
        assert_eq!(jobs_for(1), 1);
        for (cores, expected) in [(1, 1), (2, 1), (3, 2), (4, 2), (6, 4), (12, 8), (32, 21)] {
            assert_eq!(two_thirds(cores), expected, "{cores} cores");
        }
    }

    #[test]
    fn a_list_gives_the_tests_and_not_the_benchmarks_or_the_summary() {
        let names = parse_list(
            "alpha::one: test\nalpha::two: test\nspeed: benchmark\n\n3 tests, 1 benchmarks\n",
        );
        assert_eq!(
            names.into_iter().collect::<Vec<_>>(),
            ["alpha::one", "alpha::two"]
        );
    }

    #[test]
    fn the_filter_matches_a_part_of_the_name_unless_exact() {
        assert!(selected("a::b::c", None, false));
        assert!(selected("a::b::c", Some("b::"), false));
        assert!(!selected("a::b::c", Some("x"), false));
        assert!(selected("a::b::c", Some("a::b::c"), true));
        assert!(!selected("a::b::c", Some("b"), true));
    }
}
