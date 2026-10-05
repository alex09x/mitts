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
use mitts::adapter::{RustAdapter, RustBuildOptions};
use mitts::cli::{BuildArgs, Cli, Commands, ExplainArgs, RunArgs};
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
    }
}

fn handle_build(args: &BuildArgs) -> i32 {
    let opt = to_rust_options(args);
    match RustAdapter::build(&opt) {
        Ok((plan, result)) => {
            if result.success {
                println!(
                    "mitts: build finished successfully ({} actions recorded in {})",
                    plan.actions.len(),
                    plan.target_directory.display()
                );
            }
            result.exit_code
        }
        Err(err) => {
            eprintln!("mitts error: {:#}", err);
            1
        }
    }
}

fn handle_run(args: &RunArgs) -> i32 {
    let opt = to_rust_options(&args.build_args);
    match RustAdapter::build(&opt) {
        Ok((plan, result)) => {
            if !result.success {
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

            let status = Command::new(&binary)
                .args(&args.trailing_args)
                .status();

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
            eprintln!("mitts error: {:#}", err);
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
            eprintln!("mitts error: {:#}", err);
            return 1;
        }
    };

    println!("Mitts Build Plan Explanation");
    println!("============================");
    println!("Manifest:         {}", explanation.manifest_path.display());
    println!("Target Directory: {}", explanation.target_directory.display());
    println!("Profile:          {}", explanation.profile);
    println!(
        "Toolchain:        {} ({})",
        explanation.toolchain.name, explanation.toolchain.version
    );
    if let Some(ref target) = explanation.toolchain.target_triple {
        println!("Host Target:      {}", target);
    }
    println!();
    println!("Effective Compilation Units ({} total):", explanation.units.len());
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
    println!("Active Milestone: M1 (Uncached build correctness & workspace coexistence)");
    println!("Cache Policy:     Uncached execution; direct artifact compilation into isolated target");
    println!("Coexistence:      Ordinary Cargo target/ and rust-analyzer untouched");

    0
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let code = match cli.command {
        Commands::Build(ref args) => handle_build(args),
        Commands::Run(ref args) => handle_run(args),
        Commands::Explain(ref args) => handle_explain(args),
    };

    if code == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(code as u8)
    }
}
