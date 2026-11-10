/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::adapter::{BuildProgressCallback, BuildProgressEvent, RustAdapter, RustBuildOptions};
use crate::cli::diagnostic::render_compiler_diagnostics;
use crate::cli::message_format::{self, MessageFormat};
use crate::cli::terminal::TerminalGuard;
use crate::cli::ui::Ui;
use crate::cli::BuildArgs;
use crate::core::ActionKind;

pub fn to_rust_options(args: &BuildArgs) -> RustBuildOptions {
    RustBuildOptions {
        manifest_path: crate::cli::manifest_discovery::discover_manifest(&args.manifest_path),
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
        extra_args: args.extra_args.clone(),
        no_cache: args.no_cache,
        cache_dir: args.cache_dir.clone(),
        locked: args.locked,
        offline: args.offline,
        frozen: args.frozen,
        example_targets: args.example.clone(),
        target_triple: args.target.clone(),
        crate_types: args.crate_types.clone(),
        quiet: args.quiet,
        json_diagnostics: args.message_format.is_json(),
        diagnostic_format: args.message_format.diagnostic_format(),
        fast_lock: args.fast_lock,
        native: args.native,
        cargo: args.cargo,
        verbose: args.verbose,
        mode: crate::adapter::BuildMode::Build,
        progress: None,
        start_dir: None,
        exclude: args.exclude.clone(),
        bin_targets: args.bin.clone(),
        all_bins: args.bins,
        lib_only: args.lib,
        all_examples: args.examples,
        bench_targets: args.bench.clone(),
        all_benches: args.benches,
        keep_going: args.keep_going,
        ..Default::default()
    }
}

pub fn create_progress_callback(progress: Option<indicatif::ProgressBar>) -> BuildProgressCallback {
    let progress_clone = progress;
    BuildProgressCallback(std::sync::Arc::new(move |event| match event {
        BuildProgressEvent::InitTotal { total, cached } => {
            if let Some(ref pb) = progress_clone {
                Ui::set_build_progress_total(pb, total, cached);
                pb.set_message("Compiling...".to_string());
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
                println!("[mitts::compile] • {}", crate_name);
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
    }))
}

pub(crate) fn create_quiet_progress_callback() -> BuildProgressCallback {
    BuildProgressCallback(std::sync::Arc::new(|event| {
        let text = match event {
            BuildProgressEvent::CompilerMessage(text) | BuildProgressEvent::Diagnostic(text) => {
                text
            }
            BuildProgressEvent::InitTotal { .. }
            | BuildProgressEvent::Compiling(_)
            | BuildProgressEvent::UnitFinished => return,
        };
        if text.ends_with('\n') {
            eprint!("{text}");
        } else {
            eprintln!("{text}");
        }
    }))
}

pub fn render_build_result(
    plan: &crate::core::BuildPlan,
    result: &crate::core::ActionResult,
    profile: &str,
    start: std::time::Instant,
    is_verbose: bool,
) -> i32 {
    let dur = format!("{:.2}s", start.elapsed().as_secs_f64());
    if result.success {
        Ui::stage_start("🔨", &format!("Compiled target [{}]", profile));
        if result.cached {
            if result.action_id == "fast-lock-hit" {
                Ui::stage_item(
                    plan.actions.is_empty(),
                    &Ui::badge_ac_hit(),
                    "Fast build lock verified: 0 project changes detected",
                );
                if let Some(bin) = plan.actions.first() {
                    Ui::stage_item(true, &Ui::badge_cached(), &bin.description);
                }
                Ui::finish(profile, &dur, "Up-to-date (no-op)");
            } else {
                Ui::stage_item(
                    true,
                    &Ui::badge_ac_hit(),
                    &format!("Restored {} actions from Action Cache", plan.actions.len()),
                );
                Ui::finish(profile, &dur, "100% cache hit, 0 local target bloat");
            }
        } else if plan.actions.len() <= 3 {
            for (i, action) in plan.actions.iter().enumerate() {
                let is_last = i == plan.actions.len() - 1;
                Ui::stage_item(is_last, &Ui::badge_compile(), &action.description);
            }
            Ui::finish(
                profile,
                &dur,
                &format!("{} actions executed into CAS", plan.actions.len()),
            );
        } else {
            let root_bins: Vec<_> = plan
                .actions
                .iter()
                .filter(|a| a.kind == ActionKind::CompileBin)
                .collect();
            let dep_count = plan.actions.len().saturating_sub(root_bins.len());
            if let Some(bin) = root_bins.first() {
                Ui::stage_item(
                    false,
                    &Ui::badge_compile(),
                    &format!(
                        "Compiled {} actions into CAS ({} dependencies)",
                        plan.actions.len(),
                        dep_count
                    ),
                );
                Ui::stage_item(true, &Ui::badge_compile(), &bin.description);
            } else {
                Ui::stage_item(
                    true,
                    &Ui::badge_compile(),
                    &format!("Compiled {} actions into CAS", plan.actions.len()),
                );
            }
            Ui::finish(
                profile,
                &dur,
                &format!("{} actions executed into CAS", plan.actions.len()),
            );
        }
        0
    } else {
        render_compiler_diagnostics(&result.stderr, is_verbose);
        result.exit_code
    }
}

pub fn handle_build(args: &BuildArgs) -> i32 {
    if !args.config.is_empty() {
        eprintln!("Error: Unsupported option: --config is not yet supported in mitts");
        return 1;
    }

    let mut opt = to_rust_options(args);
    if args.message_format != MessageFormat::Human {
        return message_format::run_non_human_build(&mut opt, args.message_format);
    }
    let start = std::time::Instant::now();
    let profile = if opt.release { "release" } else { "dev" };
    let is_verbose = opt.verbose > 0 || std::env::var("MITTS_VERBOSE").is_ok();

    if !args.quiet {
        Ui::header(&format!("build [{}]", profile));
    }
    let term_guard = TerminalGuard::acquire();
    let progress = if !is_verbose && !args.quiet {
        Some(Ui::create_build_progress(profile))
    } else {
        None
    };

    opt.progress = Some(if args.quiet {
        create_quiet_progress_callback()
    } else {
        create_progress_callback(progress.clone())
    });

    let build_res = RustAdapter::build(&opt);
    if let Some(ref pb) = progress {
        pb.finish_and_clear();
    }
    drop(term_guard);
    if std::env::var_os("MITTS_TIMINGS").is_some() || is_verbose {
        crate::core::metrics::print_timings(
            start.elapsed(),
            std::time::Duration::ZERO,
            start.elapsed(),
        );
    }

    match build_res {
        Ok((_plan, result)) if args.quiet && result.success => 0,
        Ok((plan, result)) => render_build_result(&plan, &result, profile, start, is_verbose),
        Err(err) => {
            Ui::report_error(&err);
            1
        }
    }
}

#[cfg(test)]
#[path = "build_tests.rs"]
mod tests;
