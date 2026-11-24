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
use anstream::{eprint, eprintln, println};

pub fn to_rust_options(args: &BuildArgs) -> RustBuildOptions {
    let mut extra_args = args.extra_args.clone();
    if args.workspace
        && !extra_args
            .iter()
            .any(|a| a == "--workspace" || a == "--all")
    {
        extra_args.push("--workspace".to_string());
    }
    let mut opt = RustBuildOptions {
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
        extra_args,
        no_cache: args.no_cache,
        cache_dir: args.cache_dir.clone(),
        locked: args.locked,
        offline: args.offline,
        frozen: args.frozen,
        example_targets: args.example.clone(),
        test_targets: args.test.clone(),
        all_tests: args.tests || args.all_targets,
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
        all_bins: args.bins || args.all_targets,
        lib_only: args.lib || args.all_targets,
        all_examples: args.examples || args.all_targets,
        bench_targets: args.bench.clone(),
        all_benches: args.benches || args.all_targets,
        keep_going: args.keep_going,
        ..Default::default()
    };
    crate::cli::manifest_discovery::select_workspace_member(&mut opt, args.workspace);
    opt
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
        BuildProgressEvent::Running(message) => {
            if let Some(ref pb) = progress_clone {
                pb.set_message(message);
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
            | BuildProgressEvent::Running(_)
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
                crate::cli::build_summary::print();
                Ui::finish(profile, &dur, "Up-to-date (no-op)");
            } else {
                Ui::stage_item(
                    true,
                    &Ui::badge_ac_hit(),
                    &format!("Restored {} units from the cache", plan.actions.len()),
                );
                crate::cli::build_summary::print();
                Ui::finish(profile, &dur, "100% cache hit, nothing compiled");
            }
        } else {
            // The plan lists every unit of the build, restored ones too; the counts of what
            // really ran come from the build report.
            let outcome = crate::cli::build_summary::outcome("compiled")
                .unwrap_or_else(|| format!("{} units", plan.actions.len()));
            let root_bins: Vec<_> = plan
                .actions
                .iter()
                .filter(|a| a.kind == ActionKind::CompileBin)
                .collect();
            if plan.actions.len() <= 3 {
                for (i, action) in plan.actions.iter().enumerate() {
                    let is_last = i == plan.actions.len() - 1;
                    Ui::stage_item(is_last, &Ui::badge_compile(), &action.description);
                }
            } else if let Some(bin) = root_bins.first() {
                Ui::stage_item(
                    false,
                    &Ui::badge_compile(),
                    &format!("{} units: {outcome}", plan.actions.len()),
                );
                Ui::stage_item(true, &Ui::badge_compile(), &bin.description);
            } else {
                Ui::stage_item(
                    true,
                    &Ui::badge_compile(),
                    &format!("{} units: {outcome}", plan.actions.len()),
                );
            }
            crate::cli::build_summary::print();
            Ui::finish(profile, &dur, &outcome);
        }
        0
    } else {
        render_compiler_diagnostics(&result.stderr, is_verbose);
        result.exit_code
    }
}

pub fn handle_build(args: &BuildArgs) -> i32 {
    if let Err(error) = crate::adapter::rust::cargo_config_cli::set(&args.config) {
        eprintln!("error: {error:#}");
        return 101;
    }

    let mut opt = to_rust_options(args);
    if let Some(code) = crate::cli::profile_flag::apply_or_exit(&mut opt, args.profile.as_deref()) {
        return code;
    }
    if args.message_format != MessageFormat::Human {
        let code = message_format::run_non_human_build(&mut opt, args.message_format);
        if code == 0 {
            if let Some(dir) = &args.artifact_dir {
                if let Err(error) = copy_final_artifacts(&opt, dir) {
                    eprintln!("error: {error:#}");
                    return 101;
                }
            }
        }
        return code;
    }
    let start = std::time::Instant::now();
    let profile = &crate::adapter::rust::profile::profile_name(opt.release);
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

    if let (Some(dir), Ok((_, result))) = (&args.artifact_dir, &build_res) {
        if result.success {
            if let Err(error) = copy_final_artifacts(&opt, dir) {
                eprintln!("error: {error:#}");
                return 101;
            }
        }
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

/// `--artifact-dir`: the selected packages' final binaries and libraries copied there,
/// as Cargo's unstable option does.
fn copy_final_artifacts(
    opt: &crate::adapter::RustBuildOptions,
    dir: &std::path::Path,
) -> anyhow::Result<()> {
    let mut explain_opt = opt.clone();
    explain_opt.progress = None;
    explain_opt.inspect_only = true;
    let explanation = RustAdapter::explain_plan(&explain_opt)?;
    // The plan's target directory already names an explicit `--target`.
    let out_dir =
        explanation
            .target_directory
            .join(crate::adapter::rust::profile::custom::profile_dir(
                opt.release,
            ));
    let windows = opt
        .target_triple
        .as_deref()
        .map_or(cfg!(windows), |t| t.contains("windows"));
    let apple = opt
        .target_triple
        .as_deref()
        .map_or(cfg!(target_vendor = "apple"), |t| t.contains("apple"));
    std::fs::create_dir_all(dir)?;
    // Every output lands in one flat directory: two with the same file name (a binary and
    // an example both called `demo`) would overwrite each other, so that is an error.
    let mut copied: std::collections::BTreeMap<std::path::PathBuf, std::path::PathBuf> =
        std::collections::BTreeMap::new();
    for unit in explanation
        .units
        .iter()
        .filter(|u| u.is_primary && !u.for_host)
    {
        let name = unit.target_name.replace('-', "_");
        let mut files: Vec<String> = Vec::new();
        let exe = if windows { ".exe" } else { "" };
        match unit.kind.as_str() {
            "bin" => files.push(format!("{}{exe}", unit.target_name)),
            "example" => files.push(format!("examples/{}{exe}", unit.target_name)),
            _ => {
                let mut crate_types = vec![unit.kind.clone()];
                crate_types.extend(opt.crate_types.iter().cloned());
                for crate_type in &crate_types {
                    match crate_type.as_str() {
                        "rlib" | "lib" => files.push(format!("lib{name}.rlib")),
                        "cdylib" | "dylib" => files.push(if windows {
                            format!("{name}.dll")
                        } else if apple {
                            format!("lib{name}.dylib")
                        } else {
                            format!("lib{name}.so")
                        }),
                        "staticlib" => files.push(if windows {
                            format!("{name}.lib")
                        } else {
                            format!("lib{name}.a")
                        }),
                        _ => {}
                    }
                }
            }
        }
        for file in files {
            let from = out_dir.join(&file);
            if from.is_file() {
                let name = std::path::Path::new(&file)
                    .file_name()
                    .map(std::path::PathBuf::from)
                    .unwrap_or_else(|| file.clone().into());
                if let Some(first) = copied.get(&name) {
                    if *first != from {
                        anyhow::bail!(
                            "`--artifact-dir` would get two files named `{}`: {} and {}",
                            name.display(),
                            first.display(),
                            from.display()
                        );
                    }
                    continue;
                }
                copied.insert(name.clone(), from.clone());
                std::fs::copy(&from, dir.join(name))?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "build_tests.rs"]
mod tests;
