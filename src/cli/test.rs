/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::adapter::{
    BuildMode, BuildProgressCallback, BuildProgressEvent, RustAdapter, RustBuildOptions,
};
use crate::cli::diagnostic::render_compiler_diagnostics;
use crate::cli::terminal::TerminalGuard;
use crate::cli::ui::Ui;
use clap::Args;
use std::path::PathBuf;

#[derive(Args, Debug, Clone, Default)]
pub struct TestArgs {
    /// Path to Cargo.toml
    #[arg(long)]
    pub manifest_path: Option<PathBuf>,

    /// Package with tests to run
    #[arg(short, long)]
    pub package: Option<String>,

    /// Test all packages in the workspace
    #[arg(long, visible_alias = "all")]
    pub workspace: bool,

    /// Build and run tests in release mode
    #[arg(short, long)]
    pub release: bool,

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

    /// Arguments passed to the test binary runner (e.g. -- --nocapture)
    #[arg(last = true)]
    pub test_args: Vec<String>,
}

pub fn create_test_progress_callback(
    progress: Option<indicatif::ProgressBar>,
) -> BuildProgressCallback {
    let progress_clone = progress;
    BuildProgressCallback(std::sync::Arc::new(move |event| match event {
        BuildProgressEvent::InitTotal { total, cached } => {
            if let Some(ref pb) = progress_clone {
                Ui::set_build_progress_total(pb, total, cached);
                pb.set_message("Compiling tests...".to_string());
            } else {
                println!(
                    "[mitts::dag] Initialized DAG: {} units ({} pre-cached)",
                    total, cached
                );
            }
        }
        BuildProgressEvent::Compiling(crate_name) => {
            if let Some(ref pb) = progress_clone {
                pb.set_message(format!("• {}", crate_name));
            } else {
                println!("[mitts::test] • {}", crate_name);
            }
        }
        BuildProgressEvent::UnitFinished => {
            if let Some(ref pb) = progress_clone {
                if pb.position() >= pb.length().unwrap_or(0) {
                    pb.set_length(pb.position() + 1);
                }
                pb.inc(1);
            }
        }
        BuildProgressEvent::CompilerMessage(rendered) => {
            if let Some(ref pb) = progress_clone {
                pb.println(rendered);
            } else {
                println!("{}", rendered);
            }
        }
        BuildProgressEvent::Diagnostic(line) => {
            if let Some(ref pb) = progress_clone {
                pb.println(line);
            } else {
                println!("{}", line);
            }
        }
    }))
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
            release: args.release,
            target_dir: args.target_dir.clone(),
            features: args.features.clone(),
            no_default_features: args.no_default_features,
            all_features: args.all_features,
            no_cache: args.no_cache,
            cache_dir: args.cache_dir.clone(),
            filter: args.filter.clone(),
            verbose: args.verbose,
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
        package: args.package.clone(),
        release: args.release,
        target_dir: args.target_dir.clone(),
        features: args.features.clone(),
        no_default_features: args.no_default_features,
        all_features: args.all_features,
        extra_args,
        no_cache: args.no_cache,
        cache_dir: args.cache_dir.clone(),
        native: true,
        cargo: false,
        verbose: args.verbose,
        mode: BuildMode::Test,
        progress: None,
        start_dir: None,
    };

    let start = std::time::Instant::now();
    let profile = if opt.release { "release" } else { "dev" };
    let is_verbose = opt.verbose > 0 || std::env::var("MITTS_VERBOSE").is_ok();

    Ui::header(&format!("test [{}]", profile));
    let term_guard = TerminalGuard::acquire();
    let progress = if !is_verbose {
        Some(Ui::create_build_progress(profile))
    } else {
        None
    };

    opt.progress = Some(create_test_progress_callback(progress.clone()));

    let build_res = RustAdapter::build(&opt);
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
        return result.exit_code;
    }

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

    let (binaries, declared_test_count) =
        super::test_runner::discover_test_binaries(&plan, &opt, &root_dir);

    super::test_runner::execute_test_binaries(
        binaries,
        declared_test_count,
        args,
        &root_dir,
        start,
        profile,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_test_progress_callback_all_events() {
        let cb_none = create_test_progress_callback(None);
        cb_none.0(BuildProgressEvent::InitTotal {
            total: 3,
            cached: 1,
        });
        cb_none.0(BuildProgressEvent::Compiling("test_crate".to_string()));
        cb_none.0(BuildProgressEvent::UnitFinished);
        cb_none.0(BuildProgressEvent::CompilerMessage("msg".to_string()));
        cb_none.0(BuildProgressEvent::Diagnostic("diag".to_string()));

        let pb = Ui::create_build_progress("dev");
        let cb_some = create_test_progress_callback(Some(pb.clone()));
        cb_some.0(BuildProgressEvent::InitTotal {
            total: 3,
            cached: 1,
        });
        cb_some.0(BuildProgressEvent::Compiling("test_crate".to_string()));
        cb_some.0(BuildProgressEvent::UnitFinished);
        cb_some.0(BuildProgressEvent::CompilerMessage("msg".to_string()));
        cb_some.0(BuildProgressEvent::Diagnostic("diag".to_string()));
        pb.finish_and_clear();
    }
}
