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
use crate::cli::terminal::TerminalGuard;
use crate::cli::ui::Ui;
use crate::cli::BuildArgs;
use crate::core::ActionKind;

pub fn to_rust_options(args: &BuildArgs) -> RustBuildOptions {
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
        mode: crate::adapter::BuildMode::Build,
        progress: None,
        start_dir: None,
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

    opt.progress = Some(create_progress_callback(progress.clone()));

    let build_res = RustAdapter::build(&opt);
    if let Some(ref pb) = progress {
        pb.finish_and_clear();
    }
    drop(term_guard);

    match build_res {
        Ok((plan, result)) => render_build_result(&plan, &result, profile, start, is_verbose),
        Err(err) => {
            Ui::report_error(&err);
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::action::{Action, ActionKind, ActionResult, BuildPlan, ToolchainInfo};
    use std::collections::HashMap;
    use std::path::PathBuf;

    #[test]
    fn test_progress_callback_all_events() {
        // 1. None (verbose/text mode)
        let cb_none = create_progress_callback(None);
        cb_none.0(BuildProgressEvent::InitTotal {
            total: 5,
            cached: 2,
        });
        cb_none.0(BuildProgressEvent::Compiling("my_crate".to_string()));
        cb_none.0(BuildProgressEvent::UnitFinished);
        cb_none.0(BuildProgressEvent::CompilerMessage("msg".to_string()));
        cb_none.0(BuildProgressEvent::Diagnostic("diag".to_string()));

        // 2. Some (progress bar mode)
        let pb = Ui::create_build_progress("dev");
        let cb_some = create_progress_callback(Some(pb.clone()));
        cb_some.0(BuildProgressEvent::InitTotal {
            total: 5,
            cached: 2,
        });
        cb_some.0(BuildProgressEvent::Compiling("my_crate".to_string()));
        cb_some.0(BuildProgressEvent::UnitFinished);
        cb_some.0(BuildProgressEvent::CompilerMessage("msg".to_string()));
        cb_some.0(BuildProgressEvent::Diagnostic("diag".to_string()));
        pb.finish_and_clear();
    }

    #[test]
    fn test_render_build_result_branches() {
        let dummy_tc = ToolchainInfo {
            name: "rustc".to_string(),
            version: "1.80.0".to_string(),
            target_triple: None,
        };
        let make_action = |id: &str, kind: ActionKind| Action {
            id: id.to_string(),
            kind,
            description: format!("action {}", id),
            toolchain: dummy_tc.clone(),
            inputs: Vec::new(),
            outputs: vec![PathBuf::from(format!("/out/{}", id))],
            args: Vec::new(),
            env: HashMap::new(),
            dependencies: Vec::new(),
        };

        let now = std::time::Instant::now();

        // 1. Fast lock hit
        let plan_fast = BuildPlan {
            actions: vec![make_action("root-bin", ActionKind::CompileBin)],
            target_directory: PathBuf::from("/target"),
        };
        let res_fast = ActionResult {
            action_id: "fast-lock-hit".to_string(),
            exit_code: 0,
            success: true,
            stdout: String::new(),
            stderr: String::new(),
            cached: true,
        };
        assert_eq!(
            render_build_result(&plan_fast, &res_fast, "dev", now, false),
            0
        );

        // 2. Cached non-fast-lock
        let res_cached = ActionResult {
            action_id: "ac-hit".to_string(),
            exit_code: 0,
            success: true,
            stdout: String::new(),
            stderr: String::new(),
            cached: true,
        };
        assert_eq!(
            render_build_result(&plan_fast, &res_cached, "dev", now, false),
            0
        );

        // 3. actions <= 3 executed
        let res_exec = ActionResult {
            action_id: "exec".to_string(),
            exit_code: 0,
            success: true,
            stdout: String::new(),
            stderr: String::new(),
            cached: false,
        };
        assert_eq!(
            render_build_result(&plan_fast, &res_exec, "dev", now, false),
            0
        );

        // 4. actions > 3 executed with root bin
        let plan_many = BuildPlan {
            actions: vec![
                make_action("dep1", ActionKind::CompileRlib),
                make_action("dep2", ActionKind::CompileRlib),
                make_action("dep3", ActionKind::CompileRlib),
                make_action("root", ActionKind::CompileBin),
            ],
            target_directory: PathBuf::from("/target"),
        };
        assert_eq!(
            render_build_result(&plan_many, &res_exec, "dev", now, false),
            0
        );

        // 5. actions > 3 without root bin
        let plan_libs = BuildPlan {
            actions: vec![
                make_action("dep1", ActionKind::CompileRlib),
                make_action("dep2", ActionKind::CompileRlib),
                make_action("dep3", ActionKind::CompileRlib),
                make_action("dep4", ActionKind::CompileRlib),
            ],
            target_directory: PathBuf::from("/target"),
        };
        assert_eq!(
            render_build_result(&plan_libs, &res_exec, "dev", now, false),
            0
        );

        // 6. failure
        let res_fail = ActionResult {
            action_id: "fail".to_string(),
            exit_code: 101,
            success: false,
            stdout: String::new(),
            stderr: "error: test failure\n".to_string(),
            cached: false,
        };
        assert_eq!(
            render_build_result(&plan_fast, &res_fail, "dev", now, true),
            101
        );
    }
}
