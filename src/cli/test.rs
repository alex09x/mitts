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
use crate::core::ActionKind;
use clap::Args;
use std::path::PathBuf;
use std::process::Command;

#[derive(Args, Debug, Clone, Default)]
pub struct TestArgs {
    /// Path to Cargo.toml
    #[arg(long)]
    pub manifest_path: Option<PathBuf>,

    /// Package with tests to run
    #[arg(short, long)]
    pub package: Option<String>,

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

    /// Arguments passed to the test binary runner (e.g. -- --nocapture)
    #[arg(last = true)]
    pub test_args: Vec<String>,
}

pub fn create_test_progress_callback(
    progress: Option<indicatif::ProgressBar>,
) -> BuildProgressCallback {
    let progress_clone = progress;
    BuildProgressCallback(std::sync::Arc::new(
        move |event| match event {
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
        },
    ))
}

pub fn handle_test(args: &TestArgs) -> i32 {
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

    let mut opt = RustBuildOptions {
        manifest_path: args.manifest_path.clone(),
        package: args.package.clone(),
        release: args.release,
        target_dir: args.target_dir.clone(),
        features: args.features.clone(),
        no_default_features: args.no_default_features,
        all_features: args.all_features,
        extra_args: Vec::new(),
        no_cache: args.no_cache,
        cache_dir: args.cache_dir.clone(),
        native: true,
        cargo: false,
        verbose: args.verbose,
        mode: BuildMode::Test,
        progress: None,
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

    // Locate all compiled test binaries from plan actions
    let mut test_binaries = Vec::new();
    for action in &plan.actions {
        if action.kind == ActionKind::CompileBin && action.id.contains("::test:") {
            for out in &action.outputs {
                if out.is_file() {
                    let ext = out.extension().and_then(|e| e.to_str()).unwrap_or("");
                    if ext == "d" || ext == "rmeta" || ext == "rlib" || ext == "pdb" {
                        continue;
                    }
                    test_binaries.push((action.description.clone(), out.clone()));
                }
            }
        }
    }

    // Also scan profile directory for test binaries if actions didn't list them
    let profile_name = if opt.release { "release" } else { "debug" };
    let profile_dir = plan.target_directory.join(profile_name);
    if test_binaries.is_empty() && profile_dir.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&profile_dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_file() {
                    let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
                    if ext == "d" || ext == "rmeta" || ext == "rlib" || ext == "pdb" {
                        continue;
                    }
                    let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    if name.contains("test") {
                        test_binaries.push((name.to_string(), p));
                    }
                }
            }
        }
    }

    test_binaries.sort_by(|a, b| a.1.cmp(&b.1));
    test_binaries.dedup_by(|a, b| a.1 == b.1);

    if args.no_run {
        let dur = format!("{:.2}s", start.elapsed().as_secs_f64());
        Ui::stage_start("🧪", &format!("Compiled tests [{}]", profile));
        Ui::stage_item(
            true,
            &Ui::badge_compile(),
            &format!("Compiled {} test binaries (--no-run)", test_binaries.len()),
        );
        Ui::finish(
            profile,
            &dur,
            &format!("{} test binaries ready", test_binaries.len()),
        );
        return 0;
    }

    if test_binaries.is_empty() {
        println!("mitts test: no tests found to run");
        return 0;
    }

    println!();
    for (desc, bin_path) in &test_binaries {
        let mut cmd = Command::new(bin_path);
        if let Some(ref filter) = args.filter {
            cmd.arg(filter);
        }
        for extra in &args.test_args {
            cmd.arg(extra);
        }

        let status = match cmd.status() {
            Ok(st) => st,
            Err(err) => {
                eprintln!("mitts error executing {}: {}", bin_path.display(), err);
                return 1;
            }
        };

        if !status.success() {
            let code = status.code().unwrap_or(1);
            let dur = format!("{:.2}s", start.elapsed().as_secs_f64());
            Ui::finish_failed(profile, &dur, &format!("Test harness failed: {}", desc));
            return code;
        }
    }

    let dur = format!("{:.2}s", start.elapsed().as_secs_f64());
    Ui::stage_start("🧪", &format!("Completed tests [{}]", profile));
    Ui::finish(
        profile,
        &dur,
        &format!(
            "All tests passed ({} test binaries executed)",
            test_binaries.len()
        ),
    );
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_test_progress_callback_all_events() {
        let cb_none = create_test_progress_callback(None);
        cb_none.0(BuildProgressEvent::InitTotal { total: 3, cached: 1 });
        cb_none.0(BuildProgressEvent::Compiling("test_crate".to_string()));
        cb_none.0(BuildProgressEvent::UnitFinished);
        cb_none.0(BuildProgressEvent::CompilerMessage("msg".to_string()));
        cb_none.0(BuildProgressEvent::Diagnostic("diag".to_string()));

        let pb = Ui::create_build_progress("dev");
        let cb_some = create_test_progress_callback(Some(pb.clone()));
        cb_some.0(BuildProgressEvent::InitTotal { total: 3, cached: 1 });
        cb_some.0(BuildProgressEvent::Compiling("test_crate".to_string()));
        cb_some.0(BuildProgressEvent::UnitFinished);
        cb_some.0(BuildProgressEvent::CompilerMessage("msg".to_string()));
        cb_some.0(BuildProgressEvent::Diagnostic("diag".to_string()));
        pb.finish_and_clear();
    }
}
