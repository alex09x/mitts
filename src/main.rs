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
use mitts::adapter::{
    BuildProgressCallback, BuildProgressEvent, FetchOptions, RustAdapter, RustBuildOptions,
};
use mitts::cli::{BuildArgs, Cli, Commands, ExplainArgs, FetchArgs, GcArgs, RunArgs, Ui};
use mitts::core::{
    ActionCache, ActionKind, CacheGc, ContentAddressedStore, GcOptions, SourceStore,
};
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
        progress: None,
    }
}

fn handle_build(args: &BuildArgs) -> i32 {
    let mut opt = to_rust_options(args);
    let start = std::time::Instant::now();
    let profile = if opt.release { "release" } else { "dev" };

    Ui::header(&format!("build [{}]", profile));
    let spinner = Ui::create_spinner(&format!("Compiling target [{}]...", profile));

    let spinner_clone = spinner.clone();
    let profile_str = profile.to_string();
    opt.progress = Some(BuildProgressCallback(std::sync::Arc::new(move |event| {
        match event {
            BuildProgressEvent::Compiling(crate_name) => {
                spinner_clone.set_message(format!(
                    "Compiling target [{}] • {}",
                    profile_str, crate_name
                ));
            }
            BuildProgressEvent::CompilerMessage(rendered) => {
                spinner_clone.println(rendered);
            }
            BuildProgressEvent::Diagnostic(line) => {
                spinner_clone.println(line);
            }
        }
    })));

    match RustAdapter::build(&opt) {
        Ok((plan, result)) => {
            spinner.finish_and_clear();
            Ui::stage_start("🔨", &format!("Compiled target [{}]", profile));
            let dur = format!("{:.2}s", start.elapsed().as_secs_f64());
            if result.success {
                if result.cached {
                    Ui::stage_item(
                        true,
                        &Ui::badge_ac_hit(),
                        &format!("Restored {} actions from Action Cache", plan.actions.len()),
                    );
                    Ui::finish(profile, &dur, "100% cache hit, 0 local target bloat");
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
                Ui::report_message(
                    &format!("Compilation failed with exit code {}", result.exit_code),
                    Some("Check the compiler error diagnostics above for details."),
                );
            }
            result.exit_code
        }
        Err(err) => {
            spinner.finish_and_clear();
            Ui::report_error(&err);
            1
        }
    }
}

fn handle_run(args: &RunArgs) -> i32 {
    let mut opt = to_rust_options(&args.build_args);
    let profile = if opt.release { "release" } else { "dev" };
    let spinner = Ui::create_spinner(&format!("Compiling target [{}]...", profile));
    let spinner_clone = spinner.clone();
    let profile_str = profile.to_string();
    opt.progress = Some(BuildProgressCallback(std::sync::Arc::new(move |event| {
        match event {
            BuildProgressEvent::Compiling(crate_name) => {
                spinner_clone.set_message(format!(
                    "Compiling target [{}] • {}",
                    profile_str, crate_name
                ));
            }
            BuildProgressEvent::CompilerMessage(rendered) => {
                spinner_clone.println(rendered);
            }
            BuildProgressEvent::Diagnostic(line) => {
                spinner_clone.println(line);
            }
        }
    })));

    let build_res = RustAdapter::build(&opt);
    spinner.finish_and_clear();

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

fn handle_explain(args: &ExplainArgs) -> i32 {
    let opt = RustBuildOptions {
        manifest_path: args.manifest_path.clone(),
        package: args.package.clone(),
        release: args.release,
        features: args.features.clone(),
        no_default_features: args.no_default_features,
        all_features: args.all_features,
        ..Default::default()
    };

    let explanation = match RustAdapter::explain_plan(&opt) {
        Ok(exp) => exp,
        Err(err) => {
            Ui::report_error(&err);
            return 1;
        }
    };

    println!("Mitts Build Plan Explanation");
    println!("============================");
    println!("Manifest:         {}", explanation.manifest_path.display());
    println!(
        "Target Directory: {}",
        explanation.target_directory.display()
    );
    println!("Profile:          {}", explanation.profile);
    println!(
        "Toolchain:        {} ({})",
        explanation.toolchain.name, explanation.toolchain.version
    );
    if let Some(ref target) = explanation.toolchain.target_triple {
        println!("Host Target:      {}", target);
    }
    println!();
    println!(
        "Effective Compilation Units ({} total):",
        explanation.units.len()
    );
    for (i, unit) in explanation.units.iter().enumerate() {
        println!(
            "  {}. {} v{} [{}: {}]",
            i + 1,
            unit.package_name,
            unit.version,
            unit.kind,
            unit.target_name
        );
        println!("     Source:      {}", unit.src_path.display());
        println!("     Eligibility: {}", unit.cache_eligibility);
    }
    println!();
    println!("Active Milestone: M3 (Fast parallel fetcher & Cargo.lock resolution)");
    println!("Cache Policy:     Immutable CAS, Action Cache, and Global Source Store");
    println!("Coexistence:      Ordinary Cargo target/ and rust-analyzer untouched");

    0
}

fn handle_fetch(args: &FetchArgs) -> i32 {
    let opt = RustBuildOptions {
        manifest_path: args.manifest_path.clone(),
        ..Default::default()
    };
    let manifest_path = match RustAdapter::resolve_manifest(&opt) {
        Ok(p) => p,
        Err(err) => {
            Ui::report_error(&err);
            return 1;
        }
    };

    let store = match args.cache_dir {
        Some(ref cd) => match SourceStore::new(cd.join("sources")) {
            Ok(s) => s,
            Err(e) => {
                Ui::report_error(&e);
                return 1;
            }
        },
        None => match SourceStore::default_store() {
            Ok(s) => s,
            Err(e) => {
                Ui::report_error(&e);
                return 1;
            }
        },
    };

    let fetch_opt = FetchOptions {
        offline: args.offline,
        locked: args.locked,
        frozen: args.frozen,
    };

    Ui::header("fetch");
    Ui::stage_start(
        "🔍",
        &format!("Resolving dependencies for {}", manifest_path.display()),
    );

    match RustAdapter::fetch(&manifest_path, &store, &fetch_opt) {
        Ok(result) => {
            Ui::stage_step("📦", "Synchronizing packages with registry");
            Ui::stage_item(
                false,
                &Ui::badge_cached(),
                &format!(
                    "{} packages already present in registry",
                    result.already_cached
                ),
            );
            Ui::stage_item(
                true,
                &Ui::badge_download(),
                &format!(
                    "{} packages downloaded & verified (parallel)",
                    result.fetched
                ),
            );
            Ui::finish(
                "sources",
                &format!("{}ms", result.duration_ms),
                &format!("registry: {}", store.root().display()),
            );
            0
        }
        Err(err) => {
            Ui::report_error(&err);
            1
        }
    }
}

fn handle_gc(args: &GcArgs) -> i32 {
    let cas = match args.cache_dir {
        Some(ref cd) => match ContentAddressedStore::new(cd.join("cas")) {
            Ok(s) => s,
            Err(e) => {
                Ui::report_error(&e);
                return 1;
            }
        },
        None => match ContentAddressedStore::default_store() {
            Ok(s) => s,
            Err(e) => {
                Ui::report_error(&e);
                return 1;
            }
        },
    };

    let ac = match args.cache_dir {
        Some(ref cd) => match ActionCache::new(cd.join("ac")) {
            Ok(c) => c,
            Err(e) => {
                Ui::report_error(&e);
                return 1;
            }
        },
        None => match ActionCache::default_cache() {
            Ok(c) => c,
            Err(e) => {
                Ui::report_error(&e);
                return 1;
            }
        },
    };

    let gc = CacheGc::new(&cas, &ac);
    let opts = GcOptions {
        max_bytes: args.max_bytes,
        max_age: args.max_age_secs.map(std::time::Duration::from_secs),
    };

    match gc.prune(&opts) {
        Ok(report) => {
            println!(
                "mitts: GC completed — {} examined, {} pruned ({} bytes freed, {} bytes retained)",
                report.objects_examined,
                report.objects_pruned,
                report.bytes_freed,
                report.bytes_retained
            );
            0
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
    };

    if code == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(code as u8)
    }
}
