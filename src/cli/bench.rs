/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! `mitts bench`: compile the benchmark targets of the selected packages in the `bench` profile
//! (the release profile plus `[profile.bench]`) and run them with `--bench`, the argument Cargo
//! gives a benchmark binary to measure instead of test. It reuses the test pipeline; the
//! differences are the selection (benchmark targets only), the profile, the argument and that
//! no documentation tests run.

use crate::cli::message_format::MessageFormat;
use crate::cli::test::{handle_test, TestArgs};
use clap::Args;
use std::path::PathBuf;

#[derive(Args, Debug, Clone, Default)]
pub struct BenchArgs {
    /// Path to Cargo.toml
    #[arg(long)]
    pub manifest_path: Option<PathBuf>,

    /// Limit concurrent native build units (same as MITTS_JOBS; the flag wins)
    #[arg(short = 'j', long, value_name = "N", value_parser = clap::value_parser!(u64).range(1..))]
    pub jobs: Option<u64>,

    /// Suppress Mitts banners and stage summaries
    #[arg(short = 'q', long)]
    pub quiet: bool,

    /// Diagnostic output format: human, short, json, or Cargo json-* variants
    #[arg(long, default_value_t = MessageFormat::Human, value_name = "FMT")]
    pub message_format: MessageFormat,

    /// Packages to benchmark (repeat for multiple packages)
    #[arg(short = 'p', long = "package", action = clap::ArgAction::Append)]
    pub packages: Vec<String>,

    /// Benchmark all packages in the workspace
    #[arg(long, visible_alias = "all")]
    pub workspace: bool,

    /// Exclude packages from being benchmarked
    #[arg(long, value_name = "SPEC", action = clap::ArgAction::Append)]
    pub exclude: Vec<String>,

    /// Benchmark only the named benchmark target (repeatable); default is every benchmark target
    #[arg(long = "bench", value_name = "NAME", action = clap::ArgAction::Append)]
    pub bench: Vec<String>,

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

    /// Compile the benchmarks, but don't run them
    #[arg(long)]
    pub no_run: bool,

    /// Run all benchmark targets regardless of failure (default is fail-fast)
    #[arg(long)]
    pub no_fail_fast: bool,

    /// -v: every rustc command, cache hits, compiler path, slowest units; -vv: also their environment and build-script output
    #[arg(short, long, action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// Override a Cargo configuration value: TOML KEY=VALUE (e.g. build.jobs=2) or a TOML file
    #[arg(long, action = clap::ArgAction::Append)]
    pub config: Vec<String>,

    /// Benchmark name filter, passed to the benchmark binaries
    #[arg(value_name = "BENCHNAME")]
    pub filter: Option<String>,

    /// Arguments passed to the benchmark binaries after `--`
    #[arg(last = true)]
    pub bench_args: Vec<String>,
}

pub fn handle_bench(args: &BenchArgs) -> i32 {
    handle_test(&test_args(args))
}

/// The `mitts test` invocation that `mitts bench` stands for.
fn test_args(args: &BenchArgs) -> TestArgs {
    let mut passed = vec!["--bench".to_string()];
    passed.extend(args.bench_args.iter().cloned());
    TestArgs {
        manifest_path: crate::cli::manifest_discovery::discover_manifest(&args.manifest_path),
        jobs: args.jobs,
        quiet: args.quiet,
        message_format: args.message_format,
        packages: args.packages.clone(),
        workspace: args.workspace,
        exclude: args.exclude.clone(),
        release: true,
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
        verbose: args.verbose,
        config: args.config.clone(),
        filter: args.filter.clone(),
        // Every benchmark target unless some are named.
        benches: args.bench.is_empty(),
        bench_names: args.bench.clone(),
        bench_mode: true,
        test_args: passed,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bench_runs_the_benchmark_targets_in_release_with_the_bench_argument() {
        let args = BenchArgs {
            bench_args: vec!["--save-baseline".into(), "main".into()],
            filter: Some("fast".into()),
            ..Default::default()
        };
        let test = test_args(&args);
        assert!(test.release && test.benches && test.bench_mode);
        assert!(test.bench_names.is_empty());
        assert_eq!(test.test_args, ["--bench", "--save-baseline", "main"]);
        assert_eq!(test.filter.as_deref(), Some("fast"));
    }

    #[test]
    fn named_benchmarks_replace_the_all_benchmarks_selection() {
        let args = BenchArgs {
            bench: vec!["throughput".into()],
            ..Default::default()
        };
        let test = test_args(&args);
        assert!(!test.benches);
        assert_eq!(test.bench_names, ["throughput"]);
    }
}
