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
pub struct CheckArgs {
    /// Path to Cargo.toml
    #[arg(long)]
    pub manifest_path: Option<PathBuf>,

    /// Package to check
    #[arg(short, long)]
    pub package: Option<String>,

    /// Check artifacts in release mode
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

    /// Use verbose output (-v, -vv for very verbose)
    #[arg(short, long, action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// Override a configuration value (not yet supported)
    #[arg(long, action = clap::ArgAction::Append)]
    pub config: Vec<String>,

    /// Additional arguments passed to the compiler
    #[arg(last = true)]
    pub extra_args: Vec<String>,
}

pub fn handle_check(args: &CheckArgs) -> i32 {
    if !args.config.is_empty() {
        eprintln!("Error: Unsupported option: --config is not yet supported in mitts");
        return 1;
    }

    let mut opt = RustBuildOptions {
        manifest_path: args.manifest_path.clone(),
        package: args.package.clone(),
        release: args.release,
        target_dir: args.target_dir.clone(),
        features: args.features.clone(),
        no_default_features: args.no_default_features,
        all_features: args.all_features,
        extra_args: args.extra_args.clone(),
        no_cache: args.no_cache,
        cache_dir: args.cache_dir.clone(),
        native: true,
        cargo: false,
        verbose: args.verbose,
        mode: BuildMode::Check,
        progress: None,
        start_dir: None,
    };

    let start = std::time::Instant::now();
    let profile = if opt.release { "release" } else { "dev" };
    let is_verbose = opt.verbose > 0 || std::env::var("MITTS_VERBOSE").is_ok();

    Ui::header(&format!("check [{}]", profile));
    let term_guard = TerminalGuard::acquire();
    let progress = if !is_verbose {
        Some(Ui::create_build_progress(profile))
    } else {
        None
    };

    let progress_clone = progress.clone();
    opt.progress = Some(BuildProgressCallback(std::sync::Arc::new(
        move |event| match event {
            BuildProgressEvent::InitTotal { total, cached } => {
                if let Some(ref pb) = progress_clone {
                    Ui::set_build_progress_total(pb, total, cached);
                    pb.set_message("Checking...".to_string());
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
                    println!("[mitts::check] • {}", crate_name);
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
                    if pb.is_hidden() {
                        if rendered.ends_with('\n') {
                            eprint!("{}", rendered);
                        } else {
                            eprintln!("{}", rendered);
                        }
                    } else {
                        pb.println(rendered);
                    }
                } else if rendered.ends_with('\n') {
                    eprint!("{}", rendered);
                } else {
                    eprintln!("{}", rendered);
                }
            }
            BuildProgressEvent::Diagnostic(line) => {
                if let Some(ref pb) = progress_clone {
                    if pb.is_hidden() {
                        if line.ends_with('\n') {
                            eprint!("{}", line);
                        } else {
                            eprintln!("{}", line);
                        }
                    } else {
                        pb.println(line);
                    }
                } else if line.ends_with('\n') {
                    eprint!("{}", line);
                } else {
                    eprintln!("{}", line);
                }
            }
        },
    )));

    let build_res = RustAdapter::build(&opt);
    if let Some(ref pb) = progress {
        pb.finish_and_clear();
    }
    drop(term_guard);

    match build_res {
        Ok((plan, result)) => {
            let dur = format!("{:.2}s", start.elapsed().as_secs_f64());
            if result.success {
                if opt.progress.is_none() && !result.stderr.is_empty() {
                    if result.stderr.ends_with('\n') {
                        eprint!("{}", result.stderr);
                    } else {
                        eprintln!("{}", result.stderr);
                    }
                }
                Ui::stage_start("🔎", &format!("Checked target [{}]", profile));
                if result.cached {
                    if result.action_id == "fast-lock-hit" {
                        Ui::stage_item(
                            true,
                            &Ui::badge_ac_hit(),
                            "Fast build lock verified: 0 project changes detected",
                        );
                        Ui::finish(profile, &dur, "Up-to-date (no-op)");
                    } else {
                        Ui::stage_item(
                            true,
                            &Ui::badge_ac_hit(),
                            &format!(
                                "Restored {} check actions from Action Cache",
                                plan.actions.len()
                            ),
                        );
                        Ui::finish(profile, &dur, "100% cache hit, 0 local target bloat");
                    }
                } else {
                    Ui::stage_item(
                        true,
                        &Ui::badge_compile(),
                        &format!(
                            "Checked {} actions into CAS (0 binary bloat)",
                            plan.actions.len()
                        ),
                    );
                    Ui::finish(
                        profile,
                        &dur,
                        &format!("{} actions checked with 0 errors", plan.actions.len()),
                    );
                }
            } else {
                if !result.stderr.is_empty() {
                    if result.stderr.ends_with('\n') {
                        eprint!("{}", result.stderr);
                    } else {
                        eprintln!("{}", result.stderr);
                    }
                }
                Ui::stage_start("❌", &format!("Check failed [{}]", profile));
                render_compiler_diagnostics(&result.stderr, is_verbose);
                Ui::finish_failed(profile, &dur, "type check errors");
            }
            result.exit_code
        }
        Err(err) => {
            Ui::report_error(&err);
            1
        }
    }
}
