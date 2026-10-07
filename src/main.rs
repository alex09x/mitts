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
use mitts::cli::{
    handle_build, handle_check, handle_clean, handle_clippy, handle_coverage, handle_explain,
    handle_fetch, handle_fmt, handle_gc, handle_run, handle_test, Cli, Commands,
};
use std::process::ExitCode;

fn main() -> ExitCode {
    let cli = Cli::parse();
    let code = match cli.command {
        Commands::Build(ref args) => handle_build(args),
        Commands::Check(ref args) => handle_check(args),
        Commands::Clippy(ref args) => handle_clippy(args),
        Commands::Fmt(ref args) => handle_fmt(args),
        Commands::Test(ref args) => handle_test(args),
        Commands::Coverage(ref args) => handle_coverage(args),
        Commands::LlvmCov(ref args) => handle_coverage(args),
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
