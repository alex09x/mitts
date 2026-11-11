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
    handle_bench, handle_build, handle_check, handle_clean, handle_clippy, handle_coverage,
    handle_explain, handle_fetch, handle_fmt, handle_gc, handle_metadata, handle_package,
    handle_publish, handle_run, handle_rustc, handle_test, Cli, Commands,
};
use std::process::ExitCode;

fn main() -> ExitCode {
    // Started by rustc as the linker of a unit whose link time is measured.
    if let Some(code) = mitts::adapter::rust::link_timing::run_as_linker_if_requested() {
        return ExitCode::from(u8::try_from(code).unwrap_or(1));
    }
    // `mitts +nightly build`, like `cargo +nightly build`.
    let args = match mitts::adapter::rust::toolchain_select::split_toolchain_arg(
        std::env::args_os().collect(),
    ) {
        Ok((toolchain, args)) => {
            if let Some(toolchain) = toolchain {
                if let Err(error) = mitts::adapter::rust::toolchain_select::select(&toolchain) {
                    eprintln!("error: {error}");
                    return ExitCode::from(101);
                }
            }
            args
        }
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::from(101);
        }
    };
    let cli = Cli::parse_from(args);
    mitts::cli::color::apply(mitts::cli::color::ColorWhen::resolve(cli.color));
    mitts::core::verbosity::set(cli.command.verbose());
    if mitts::core::verbosity::level() >= 1 || cli.timings.is_some() {
        mitts::adapter::rust::link_timing::enable();
    }
    if let Some(jobs) = cli.command.jobs() {
        mitts::core::jobs::set_flag_limit(jobs);
    }
    // Cargo's -Z flags select unstable Cargo features; `unstable-options` only unlocks
    // flags, which Mitts has none of behind it. rustc's own -Z flags belong in RUSTFLAGS.
    if let Some(flag) = cli
        .unstable
        .iter()
        .find(|f| f.as_str() != "unstable-options")
    {
        eprintln!(
            "error: unstable Cargo flag `-Z {flag}` is not supported by Mitts \
             (pass rustc -Z flags through RUSTFLAGS)"
        );
        return ExitCode::from(101);
    }
    if let Some(value) = &cli.timings {
        match mitts::core::unit_timings::parse_formats(value) {
            Ok(formats) => mitts::core::unit_timings::enable(formats),
            Err(error) => {
                eprintln!("error: {error:#}");
                return ExitCode::from(101);
            }
        }
    }
    let code = match cli.command {
        Commands::Build(ref args) => handle_build(args),
        Commands::Rustc(ref args) => handle_rustc(args),
        Commands::Check(ref args) => handle_check(args),
        Commands::Clippy(ref args) => handle_clippy(args),
        Commands::Fmt(ref args) => handle_fmt(args),
        Commands::Test(ref args) => handle_test(args),
        Commands::Bench(ref args) => handle_bench(args),
        Commands::Coverage(ref args) => handle_coverage(args),
        Commands::LlvmCov(ref args) => handle_coverage(args),
        Commands::Run(ref args) => handle_run(args),
        Commands::Explain(ref args) => handle_explain(args),
        Commands::Metadata(ref args) => handle_metadata(args),
        Commands::Fetch(ref args) => handle_fetch(args),
        Commands::Package(ref args) => handle_package(args),
        Commands::Publish(ref args) => handle_publish(args),
        Commands::Gc(ref args) => handle_gc(args),
        Commands::Clean(ref args) => handle_clean(args),
    };

    mitts::adapter::rust::cargo_config_cli::cleanup();
    match mitts::core::unit_timings::finish_recorded() {
        Ok(files) => {
            for file in files {
                eprintln!("      Timing report saved to {}", file.display());
            }
        }
        Err(error) => eprintln!("warning: timing report not written: {error:#}"),
    }

    // After the command has released the cache: keep the cache under its size limit.
    if let Some(cache_dir) = cli.command.cache_use() {
        mitts::core::auto_gc::run_if_due(cache_dir.as_deref());
    }

    if code == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(code as u8)
    }
}
