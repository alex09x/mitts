/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::adapter::{BuildMode, RustAdapter, RustBuildOptions};
use crate::cli::diagnostic::render_compiler_diagnostics;
use crate::cli::terminal::TerminalGuard;
pub use crate::cli::test_progress::create_test_progress_callback;
use crate::cli::ui::Ui;
use clap::Args;
use std::path::PathBuf;

#[derive(Args, Debug, Clone, Default)]
pub struct TestArgs {
    /// Path to Cargo.toml
    #[arg(long)]
    pub manifest_path: Option<PathBuf>,

    /// Package with tests to run
    #[arg(skip)]
    pub package: Option<String>,
    /// Packages to test (repeat for multiple packages)
    #[arg(short = 'p', long = "package", action = clap::ArgAction::Append)]
    pub packages: Vec<String>,

    /// Test all packages in the workspace
    #[arg(long, visible_alias = "all")]
    pub workspace: bool,

    /// Exclude packages from being tested
    #[arg(long, value_name = "SPEC", action = clap::ArgAction::Append)]
    pub exclude: Vec<String>,

    /// Build and run tests in release mode
    #[arg(short, long)]
    pub release: bool,

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

    /// Compile, but don't run tests
    #[arg(long)]
    pub no_run: bool,

    /// Filter tests by name or substring
    #[arg(value_name = "TESTNAME")]
    pub filter: Option<String>,

    /// Collect code coverage while running tests (compatible with cargo test --coverage)
    #[arg(long)]
    pub coverage: bool,

    /// Use verbose output (-v, -vv for very verbose)
    #[arg(short, long, action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// Override a configuration value (not yet supported)
    #[arg(long, action = clap::ArgAction::Append)]
    pub config: Vec<String>,

    /// Run all tests regardless of failure (default is fail-fast)
    #[arg(long)]
    pub no_fail_fast: bool,

    /// Test the named integration target (repeatable; combines with --bin)
    #[arg(long = "test", action = clap::ArgAction::Append)]
    pub test: Vec<String>,

    /// Test all integration test targets
    #[arg(long)]
    pub tests: bool,

    /// Test all benchmarks
    #[arg(long)]
    pub benches: bool,

    /// Test all targets, including examples and benchmarks
    #[arg(long)]
    pub all_targets: bool,

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

    /// Arguments passed to test binaries after `--` (for example, test-name filters)
    #[arg(last = true)]
    pub test_args: Vec<String>,
}

pub fn handle_test(args: &TestArgs) -> i32 {
    if !args.config.is_empty() {
        eprintln!("Error: Unsupported option: --config is not yet supported in mitts");
        return 1;
    }

    if args.coverage {
        let cov_args = crate::cli::coverage::CoverageArgs {
            manifest_path: args.manifest_path.clone(),
            package: args.package.clone(),
            packages: args.packages.clone(),
            workspace: args.workspace,
            exclude: args.exclude.clone(),
            release: args.release,
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
            verbose: args.verbose,
            config: args.config.clone(),
            test_args: args.test_args.clone(),
            ..Default::default()
        };
        return crate::cli::coverage::handle_coverage(&cov_args);
    }

    let mut extra_args = Vec::new();
    if args.workspace {
        extra_args.push("--workspace".to_string());
    }

    let mut opt = RustBuildOptions {
        manifest_path: args.manifest_path.clone(),
        package: args
            .package
            .clone()
            .or_else(|| args.packages.first().cloned()),
        packages: args
            .packages
            .iter()
            .skip(usize::from(args.package.is_none()))
            .cloned()
            .collect(),
        release: args.release,
        target_dir: args.target_dir.clone(),
        features: args.features.clone(),
        no_default_features: args.no_default_features,
        all_features: args.all_features,
        extra_args,
        no_cache: args.no_cache,
        cache_dir: args.cache_dir.clone(),
        target_triple: args.target.clone(),
        locked: args.locked,
        offline: args.offline,
        frozen: args.frozen,
        fast_lock: None,
        native: true,
        cargo: false,
        verbose: args.verbose,
        mode: BuildMode::Test,
        progress: None,
        start_dir: None,
        test_targets: args.test.clone(),
        all_tests: args.tests || args.all_targets,
        all_benches: args.benches || args.all_targets,
        all_examples: args.all_targets,
        bin_targets: args.bin.clone(),
        all_bins: args.bins || args.all_targets,
        lib_only: args.lib || args.all_targets,
        doc_only: args.doc,
        exclude: args.exclude.clone(),
        example_targets: Vec::new(),
        crate_types: Vec::new(),
        quiet: false,
    };

    let root_manifest = opt
        .manifest_path
        .clone()
        .unwrap_or_else(|| PathBuf::from("Cargo.toml"));
    let root_dir = {
        let p = root_manifest
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."));
        if p.as_os_str().is_empty() {
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
        } else {
            std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
        }
    };

    if let Err(err) =
        crate::adapter::rust::test_selection::validate_test_options(&opt, &root_manifest, &root_dir)
    {
        eprintln!("error: {:#}", err);
        return 101;
    }

    let start = std::time::Instant::now();
    let profile = if opt.release { "release" } else { "dev" };
    let is_verbose = opt.verbose > 0 || std::env::var("MITTS_VERBOSE").is_ok();
    let _cache_lease = crate::core::gc::CacheLease::acquire(opt.cache_dir.as_deref());
    Ui::header(&format!("test [{}]", profile));
    let term_guard = TerminalGuard::acquire();
    let progress = if !is_verbose {
        Some(Ui::create_build_progress(profile))
    } else {
        None
    };
    opt.progress = Some(create_test_progress_callback(progress.clone()));
    let build_start = std::time::Instant::now();
    let build_res = RustAdapter::build(&opt);
    let compile_duration = build_start.elapsed();
    if let Some(ref pb) = progress {
        pb.finish_and_clear();
    }
    drop(term_guard);

    let (plan, result) = match build_res {
        Ok(res) => res,
        Err(err) => {
            Ui::report_error(&err);
            return 1;
        }
    };

    if !result.success {
        let dur = format!("{:.2}s", start.elapsed().as_secs_f64());
        Ui::stage_start("❌", &format!("Test compilation failed [{}]", profile));
        render_compiler_diagnostics(&result.stderr, is_verbose);
        Ui::finish_failed(profile, &dur, "compilation errors");
        if std::env::var_os("MITTS_TIMINGS").is_some() || args.verbose > 0 {
            crate::core::metrics::print_timings(
                compile_duration,
                std::time::Duration::ZERO,
                start.elapsed(),
            );
        }
        return result.exit_code;
    }

    let test_start = std::time::Instant::now();
    let mut exit_code = 0;
    if !args.doc {
        let (binaries, declared_test_count) =
            match super::test_runner::discover_test_binaries(&plan, &opt, &root_dir) {
                Ok(res) => res,
                Err(err) => {
                    eprintln!("mitts test error: {:#}", err);
                    return 1;
                }
            };

        exit_code = super::test_runner::execute_test_binaries_with_plan(
            binaries,
            declared_test_count,
            args,
            &root_dir,
            start,
            profile,
            Some(&plan),
        );
        if exit_code != 0 && !args.no_fail_fast {
            if std::env::var_os("MITTS_TIMINGS").is_some() || args.verbose > 0 {
                crate::core::metrics::print_timings(
                    compile_duration,
                    test_start.elapsed(),
                    start.elapsed(),
                );
            }
            return exit_code;
        }
    }

    if super::test_doctest::should_run_doctests(args) {
        match super::test_doctest::execute_doctests(&opt, args, &plan, &root_dir, profile) {
            Ok(Some(doc_fail)) if exit_code == 0 => exit_code = doc_fail,
            Ok(_) => {}
            Err(err) => {
                eprintln!("mitts error executing doctests: {}", err);
                if exit_code == 0 {
                    exit_code = 101;
                }
            }
        }
    }

    if std::env::var_os("MITTS_TIMINGS").is_some() || args.verbose > 0 {
        crate::core::metrics::print_timings(
            compile_duration,
            test_start.elapsed(),
            start.elapsed(),
        );
    }

    exit_code
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::BuildProgressEvent;

    #[test]
    fn test_test_progress_callback_all_events() {
        for pb in [None, Some(Ui::create_build_progress("dev"))] {
            let cb = create_test_progress_callback(pb.clone());
            cb.0(BuildProgressEvent::InitTotal {
                total: 3,
                cached: 1,
            });
            cb.0(BuildProgressEvent::Compiling("test_crate".to_string()));
            cb.0(BuildProgressEvent::UnitFinished);
            cb.0(BuildProgressEvent::CompilerMessage("msg".to_string()));
            cb.0(BuildProgressEvent::Diagnostic("diag".to_string()));
            if let Some(p) = pb {
                p.finish_and_clear();
            }
        }
    }
}
