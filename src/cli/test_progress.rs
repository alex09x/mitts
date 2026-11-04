/*
 * Mitts Build — test build progress reporting
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */
use crate::adapter::{BuildProgressCallback, BuildProgressEvent};
use crate::cli::ui::Ui;

pub fn create_test_progress_callback(
    progress: Option<indicatif::ProgressBar>,
) -> BuildProgressCallback {
    let progress_clone = progress;
    BuildProgressCallback(std::sync::Arc::new(move |event| match event {
        BuildProgressEvent::InitTotal { total, cached } => {
            if let Some(ref pb) = progress_clone {
                Ui::set_build_progress_total(pb, total, cached);
                pb.set_message("Compiling tests...".to_string());
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
                println!("[mitts::test] • {}", crate_name);
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
    }))
}
