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

    /// Package to measure coverage for
    #[arg(short, long)]
    pub package: Option<String>,

    /// Build and run tests in release mode
    #[arg(short, long)]
    pub release: bool,

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

    /// Automatically open the generated HTML report
    #[arg(long)]
    pub open: bool,

    /// Minimum line coverage percentage (fails if below threshold)
    #[arg(long)]
    pub fail_under_lines: Option<f64>,

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
    #[arg(short = 'F', long)]
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

    /// Compile, but don't run tests or collect profiles
    #[arg(long)]
    pub no_run: bool,

    /// Run all tests regardless of failure (default is fail-fast)
    #[arg(long)]
    pub no_fail_fast: bool,

    /// Test only the specified integration test target
    #[arg(long = "test", action = clap::ArgAction::Append)]
    pub test: Vec<String>,

    /// Test all integration test targets
    #[arg(long)]
    pub tests: bool,

    /// Test only the specified binary
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

    /// Run only tests that exactly match the filter (equivalent to -- --exact)
    #[arg(long)]
    pub exact: bool,

    /// Run only ignored tests (equivalent to -- --ignored)
    #[arg(long)]
    pub ignored: bool,

    /// Use verbose output
    #[arg(short, long, action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// Override a configuration value (not yet supported)
    #[arg(long, action = clap::ArgAction::Append)]
    pub config: Vec<String>,

    /// Arguments passed to the test binary runner
    #[arg(last = true)]
    pub test_args: Vec<String>,
}
