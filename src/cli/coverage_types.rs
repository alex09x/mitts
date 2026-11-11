/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use clap::Args;
use std::path::PathBuf;

#[derive(Args, Debug, Clone, Default)]
pub struct CoverageArgs {
    /// Path to Cargo.toml
    #[arg(long)]
    pub manifest_path: Option<PathBuf>,

    /// Limit concurrent native build units (same as MITTS_JOBS; the flag wins)
    #[arg(short = 'j', long, value_name = "N", value_parser = clap::value_parser!(u64).range(1..))]
    pub jobs: Option<u64>,

    /// Package to measure coverage for
    #[arg(skip)]
    pub package: Option<String>,
    /// Packages to measure (repeat for multiple packages)
    #[arg(short = 'p', long = "package", action = clap::ArgAction::Append)]
    pub packages: Vec<String>,

    /// Build and run tests in release mode
    #[arg(short, long)]
    pub release: bool,

    /// Suppress Mitts progress and stage banners; keep errors and the coverage report
    #[arg(short = 'q', long)]
    pub quiet: bool,

    /// Only print high-level coverage summary table
    #[arg(long)]
    pub summary_only: bool,

    /// Generate HTML coverage report
    #[arg(long)]
    pub html: bool,

    /// Generate LCOV coverage report (lcov.info)
    #[arg(long)]
    pub lcov: bool,

    /// Generate JSON coverage report
    #[arg(long)]
    pub json: bool,

    /// Generate plain text coverage report
    #[arg(long)]
    pub text: bool,

    /// Output directory for HTML or LCOV reports
    #[arg(long)]
    pub output_dir: Option<PathBuf>,

    /// Write a JSON, LCOV or text report to this file instead of stdout (or lcov.info)
    #[arg(long, value_name = "FILE")]
    pub output_path: Option<PathBuf>,

    /// Automatically open the generated HTML report
    #[arg(long)]
    pub open: bool,

    /// Minimum line coverage percentage (fails if below threshold)
    #[arg(long)]
    pub fail_under_lines: Option<f64>,

    /// Exclude matching source filenames from reports and the coverage threshold
    #[arg(long)]
    pub ignore_filename_regex: Option<String>,

    /// Filter tests by name or substring
    #[arg(value_name = "TESTNAME")]
    pub filter: Option<String>,

    /// Build all targets in the workspace
    #[arg(long, visible_alias = "all")]
    pub workspace: bool,

    /// Exclude packages from being tested
    #[arg(long, value_name = "SPEC", action = clap::ArgAction::Append)]
    pub exclude: Vec<String>,

    /// Build all targets (bins, tests, benches)
    #[arg(long)]
    pub all_targets: bool,

    /// Directory for all generated artifacts
    #[arg(long)]
    pub target_dir: Option<PathBuf>,

    /// Space or comma separated list of features to activate
    #[arg(short = 'F', long, value_delimiter = ',', num_args = 1..)]
    pub features: Vec<String>,

    /// Do not activate the `default` feature
    #[arg(long)]
    pub no_default_features: bool,

    /// Activate all available features
    #[arg(long)]
    pub all_features: bool,

    /// Do not read from or write to the shared action cache
    #[arg(long)]
    pub no_cache: bool,

    /// Directory for the shared artifact and action cache
    #[arg(long)]
    pub cache_dir: Option<PathBuf>,

    /// Select target triple for cross-compilation
    #[arg(long)]
    pub target: Option<String>,

    /// Require an existing compatible Cargo.lock; do not resolve or write one.
    #[arg(long)]
    pub locked: bool,

    /// Do not use the network to resolve dependencies; use only local sources.
    #[arg(long)]
    pub offline: bool,

    /// Require a compatible Cargo.lock and use only locally available sources.
    #[arg(long)]
    pub frozen: bool,

    /// Compile, but don't run tests or collect profiles
    #[arg(long)]
    pub no_run: bool,

    /// Run all tests regardless of failure (default is fail-fast)
    #[arg(long)]
    pub no_fail_fast: bool,

    /// Test the named integration target (repeatable; combines with --bin)
    #[arg(long = "test", action = clap::ArgAction::Append)]
    pub test: Vec<String>,

    /// Test all integration test targets
    #[arg(long)]
    pub tests: bool,

    /// Test the named binary target (repeatable; combines with --test)
    #[arg(long = "bin", action = clap::ArgAction::Append)]
    pub bin: Vec<String>,

    /// Test all binary targets
    #[arg(long)]
    pub bins: bool,

    /// Test only this package's library unit tests
    #[arg(long)]
    pub lib: bool,

    /// Test only this package's library documentation
    #[arg(long)]
    pub doc: bool,

    /// Run tests without capturing output (equivalent to -- --nocapture)
    #[arg(long)]
    pub nocapture: bool,

    /// Require exact test-name matches; pass before `--`, with filters after it
    #[arg(long)]
    pub exact: bool,

    /// Run only ignored tests (equivalent to -- --ignored)
    #[arg(long)]
    pub ignored: bool,

    /// Run every test in its own process, many at a time, as `mitts test -c` does
    ///
    /// Without a number the pool is all the cores; `-c=N` runs N processes at once.
    /// Each process writes its own profile file, so the report is the same as for a serial run.
    #[arg(short = 'c', long, value_name = "N", num_args = 0..=1, require_equals = true, default_missing_value = "0")]
    pub concurrency: Option<usize>,

    /// Use verbose output
    #[arg(short, long, action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// Override a Cargo configuration value: TOML KEY=VALUE (e.g. build.jobs=2) or a TOML file
    #[arg(long, action = clap::ArgAction::Append)]
    pub config: Vec<String>,

    /// Arguments passed to test binaries after `--` (for example, test-name filters)
    #[arg(last = true)]
    pub test_args: Vec<String>,
}

impl CoverageArgs {
    /// True when stdout carries only the JSON report, so a consumer can parse it: no banner,
    /// stage lines or output of the instrumented tests may reach it (they go to stderr).
    pub fn json_report_on_stdout(&self) -> bool {
        self.json && !self.html && !self.lcov && self.output_path.is_none()
    }

    pub fn from_test_args(args: &crate::cli::test::TestArgs) -> Self {
        Self {
            manifest_path: crate::cli::manifest_discovery::discover_manifest(&args.manifest_path),
            package: args.package.clone(),
            packages: args.packages.clone(),
            workspace: args.workspace,
            exclude: args.exclude.clone(),
            release: args.release,
            quiet: args.quiet,
            target_dir: args.target_dir.clone(),
            features: args.features.clone(),
            no_default_features: args.no_default_features,
            all_features: args.all_features,
            no_cache: args.no_cache,
            cache_dir: args.cache_dir.clone(),
            target: args.target.clone(),
            locked: args.locked,
            offline: args.offline,
            frozen: args.frozen,
            no_run: args.no_run,
            no_fail_fast: args.no_fail_fast,
            filter: args.filter.clone(),
            test: args.test.clone(),
            tests: args.tests,
            all_targets: args.all_targets,
            bin: args.bin.clone(),
            bins: args.bins,
            lib: args.lib,
            doc: args.doc,
            nocapture: args.nocapture,
            exact: args.exact,
            ignored: args.ignored,
            concurrency: args.concurrency,
            verbose: args.verbose,
            config: args.config.clone(),
            test_args: args.test_args.clone(),
            ..Default::default()
        }
    }
}
