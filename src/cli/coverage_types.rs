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
    #[arg(long)]
    pub workspace: bool,

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

    /// Use verbose output
    #[arg(short, long, action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// Arguments passed to the test binary runner
    #[arg(last = true)]
    pub test_args: Vec<String>,
}
