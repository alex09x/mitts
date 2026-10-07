/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::adapter::{BuildProgressCallback, BuildProgressEvent, RustAdapter};
use crate::cli::build::to_rust_options;
use crate::cli::terminal::TerminalGuard;
use crate::cli::ui::Ui;
use crate::cli::RunArgs;
use crate::core::ActionKind;
use std::process::Command;

pub fn handle_run(args: &RunArgs) -> i32 {
    let mut opt = to_rust_options(&args.build_args);
    // Trailing arguments after -- are for the binary execution, not rustc
    opt.extra_args.clear();
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

            let trailing = if !args.trailing_args.is_empty() {
                &args.trailing_args
            } else {
                &args.build_args.extra_args
            };
            let status = Command::new(&binary).args(trailing).status();

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
