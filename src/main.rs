/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use clap::Parser;
use mitts::adapter::{BuildProgressCallback, BuildProgressEvent, RustAdapter, RustBuildOptions};
use mitts::cli::{
    handle_clean, handle_explain, handle_fetch, handle_gc, render_compiler_diagnostics, BuildArgs,
    Cli, Commands, RunArgs, TerminalGuard, Ui,
};
use mitts::core::ActionKind;
use std::process::{Command, ExitCode};

fn to_rust_options(args: &BuildArgs) -> RustBuildOptions {
    RustBuildOptions {
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
        native: args.native,
        cargo: args.cargo,
        verbose: args.verbose,
        progress: None,
    }
}

fn handle_build(args: &BuildArgs) -> i32 {
    let mut opt = to_rust_options(args);
    let start = std::time::Instant::now();
    let profile = if opt.release { "release" } else { "dev" };
    let is_verbose = opt.verbose > 0 || std::env::var("MITTS_VERBOSE").is_ok();

    Ui::header(&format!("build [{}]", profile));
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
            } else {
                Ui::stage_start("❌", &format!("Compilation failed [{}]", profile));
                render_compiler_diagnostics(&result.stderr, is_verbose);
                Ui::finish_failed(profile, &dur, "compilation errors");
            }
            result.exit_code
        }
        Err(err) => {
            Ui::report_error(&err);
            1
        }
    }
}

fn handle_run(args: &RunArgs) -> i32 {
    let mut opt = to_rust_options(&args.build_args);
    let profile = if opt.release { "release" } else { "dev" };
    let term_guard = TerminalGuard::acquire();
    let progress = Ui::create_build_progress(profile);
    let progress_clone = progress.clone();
    opt.progress = Some(BuildProgressCallback(std::sync::Arc::new(
        move |event| match event {
            BuildProgressEvent::InitTotal { total, cached } => {
                Ui::set_build_progress_total(&progress_clone, total, cached);
                progress_clone.set_message("Compiling...".to_string());
            }
            BuildProgressEvent::Compiling(crate_name) => {
                progress_clone.set_message(format!("• {}", crate_name));
            }
            BuildProgressEvent::UnitFinished => {
                if progress_clone.position() >= progress_clone.length().unwrap_or(0) {
                    progress_clone.set_length(progress_clone.position() + 1);
                }
                progress_clone.inc(1);
            }
            BuildProgressEvent::CompilerMessage(rendered) => {
                progress_clone.println(rendered);
            }
            BuildProgressEvent::Diagnostic(line) => {
                progress_clone.println(line);
            }
        },
    )));

    let build_res = RustAdapter::build(&opt);
    progress.finish_and_clear();
    drop(term_guard);

    match build_res {
        Ok((plan, result)) => {
            if !result.success {
                if !result.stderr.is_empty() {
                    eprintln!("{}", result.stderr);
                }
                return result.exit_code;
            }

            // Locate the generated binary
            let mut bin_path = None;
            for action in &plan.actions {
                if action.kind == ActionKind::CompileBin {
                    if let Some(first_output) = action.outputs.first() {
                        bin_path = Some(first_output.clone());
                        break;
                    }
                }
            }

            let Some(binary) = bin_path else {
                eprintln!("mitts error: no executable binary found in build plan");
                return 1;
            };

            let status = Command::new(&binary).args(&args.trailing_args).status();

            match status {
                Ok(st) => {
                    #[cfg(unix)]
                    {
                        use std::os::unix::process::ExitStatusExt;
                        if let Some(code) = st.code() {
                            code
                        } else if let Some(sig) = st.signal() {
                            eprintln!("mitts: process terminated by signal {}", sig);
                            128 + sig
                        } else {
                            1
                        }
                    }
                    #[cfg(not(unix))]
                    {
                        st.code().unwrap_or(1)
                    }
                }
                Err(err) => {
                    eprintln!("mitts error executing {}: {}", binary.display(), err);
                    1
                }
            }
        }
        Err(err) => {
            Ui::report_error(&err);
            1
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let code = match cli.command {
        Commands::Build(ref args) => handle_build(args),
        Commands::Run(ref args) => handle_run(args),
        Commands::Explain(ref args) => handle_explain(args),
        Commands::Fetch(ref args) => handle_fetch(args),
        Commands::Gc(ref args) => handle_gc(args),
        Commands::Clean(ref args) => handle_clean(args),
    };

    if code == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(code as u8)
    }
}
