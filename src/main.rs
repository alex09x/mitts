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
                Ok(st) => st.code().unwrap_or(0),
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
        ..Default::default()
    };

    let manifest = match RustAdapter::resolve_manifest(&opt) {
        Ok(m) => m,
        Err(err) => {
            eprintln!("mitts error: {:#}", err);
            return 1;
        }
    };

    let target_dir = match RustAdapter::resolve_target_dir(&manifest, &opt) {
        Ok(td) => td,
        Err(err) => {
            eprintln!("mitts error: {:#}", err);
            return 1;
        }
    };

    let toolchain = match RustAdapter::detect_toolchain() {
        Ok(t) => t,
        Err(err) => {
            eprintln!("mitts error: {:#}", err);
            return 1;
        }
    };

    println!("Mitts Build Plan Explanation");
    println!("============================");
    println!("Manifest:         {}", manifest.display());
    println!("Target Directory: {}", target_dir.display());
    println!("Toolchain:        {} ({})", toolchain.name, toolchain.version);
    if let Some(target) = toolchain.target_triple {
        println!("Host Target:      {}", target);
    }
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
