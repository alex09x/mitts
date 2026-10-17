/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::adapter::rust::manifest::CargoToml;
use crate::adapter::{BuildProgressCallback, BuildProgressEvent, RustAdapter, RustBuildOptions};
use crate::cli::build::to_rust_options;
use crate::cli::terminal::TerminalGuard;
use crate::cli::ui::Ui;
use crate::cli::RunArgs;
use crate::core::ActionKind;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

struct SelectedRunTarget {
    pkg_name: String,
    pkg_dir: PathBuf,
    bin_name: String,
}

fn resolve_run_target(
    opt: &RustBuildOptions,
    manifest_path: &Path,
) -> std::result::Result<SelectedRunTarget, i32> {
    let root_toml = match CargoToml::from_file(manifest_path) {
        Ok(t) => t,
        Err(err) => {
            Ui::report_error(&err);
            return Err(1);
        }
    };
    let manifest_dir = manifest_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();

    let (ws_dir, ws_toml) = match CargoToml::find_workspace_root(manifest_path, &root_toml) {
        Ok(Some((ws_man, wt))) => (
            ws_man
                .parent()
                .unwrap_or(manifest_dir.as_path())
                .to_path_buf(),
            wt,
        ),
        _ => (manifest_dir.clone(), root_toml.clone()),
    };

    let mut candidate_pkgs: Vec<(String, PathBuf, CargoToml)> = Vec::new();
    if let Some(ref req_pkg) = opt.package {
        if let Ok(members) = ws_toml.discover_workspace_members(&ws_dir) {
            for (m_name, m_dir) in members {
                if &m_name == req_pkg {
                    if let Ok(mt) = CargoToml::from_file(&m_dir.join("Cargo.toml")) {
                        candidate_pkgs.push((m_name, m_dir, mt));
                    }
                }
            }
        }
        if candidate_pkgs.is_empty() {
            eprintln!(
                "error: package `{}` not found in workspace {}",
                req_pkg,
                ws_dir.display()
            );
            return Err(101);
        }
    } else if let Some(ref p) = root_toml.package {
        candidate_pkgs.push((p.name.clone(), manifest_dir.clone(), root_toml.clone()));
    } else if let Ok(members) = ws_toml.discover_workspace_members(&ws_dir) {
        let def_members = ws_toml
            .workspace
            .as_ref()
            .map(|w| w.default_members.as_slice())
            .unwrap_or(&[]);
        for (m_name, m_dir) in members {
            if opt.exclude.iter().any(|ex| ex == &m_name) {
                continue;
            }
            if !def_members.is_empty() {
                let rel = m_dir.strip_prefix(&ws_dir).unwrap_or(&m_dir);
                let rel_str = rel.to_string_lossy();
                if !def_members
                    .iter()
                    .any(|dm| dm == &m_name || dm.trim_end_matches('/') == rel_str)
                {
                    continue;
                }
            }
            if let Ok(mt) = CargoToml::from_file(&m_dir.join("Cargo.toml")) {
                candidate_pkgs.push((m_name, m_dir, mt));
            }
        }
    }

    let mut all_bins: Vec<SelectedRunTarget> = Vec::new();
    for (p_name, p_dir, p_toml) in &candidate_pkgs {
        for t in p_toml.discover_targets(p_dir) {
            if t.kind == "bin" {
                all_bins.push(SelectedRunTarget {
                    pkg_name: p_name.clone(),
                    pkg_dir: p_dir.clone(),
                    bin_name: t.name,
                });
            }
        }
    }
    all_bins.sort_by(|a, b| {
        a.bin_name
            .cmp(&b.bin_name)
            .then_with(|| a.pkg_name.cmp(&b.pkg_name))
    });

    if opt.bin_targets.len() > 1 {
        eprintln!("error: `mitts run` can run at most one executable at a time");
        return Err(101);
    }

    if let Some(req_bin) = opt.bin_targets.first() {
        let matches: Vec<SelectedRunTarget> = all_bins
            .into_iter()
            .filter(|b| &b.bin_name == req_bin)
            .collect();
        return match matches.len() {
            1 => Ok(matches.into_iter().next().unwrap()),
            0 => {
                eprintln!("error: no bin target named `{}`", req_bin);
                Err(101)
            }
            _ => {
                eprintln!(
                    "error: `mitts run` could not determine which binary to run; multiple packages define `{}`",
                    req_bin
                );
                Err(101)
            }
        };
    }

    if candidate_pkgs.len() == 1 {
        if let Some(def_run) = candidate_pkgs[0]
            .2
            .package
            .as_ref()
            .and_then(|p| p.default_run.as_deref())
        {
            if let Some(found) = all_bins.into_iter().find(|b| b.bin_name == def_run) {
                return Ok(found);
            }
            eprintln!(
                "error: default-run target `{}` not found in package `{}`",
                def_run, candidate_pkgs[0].0
            );
            return Err(101);
        }
    }

    match all_bins.len() {
        0 => {
            eprintln!("mitts error: a bin target must be available for `mitts run`");
            Err(1)
        }
        1 => Ok(all_bins.into_iter().next().unwrap()),
        _ => {
            let names: Vec<&str> = all_bins.iter().map(|b| b.bin_name.as_str()).collect();
            eprintln!(
                "error: `mitts run` could not determine which binary to run. Use the `--bin` option or `default-run` manifest key to specify a binary.\navailable binaries: {}",
                names.join(", ")
            );
            Err(101)
        }
    }
}

pub fn handle_run(args: &RunArgs) -> i32 {
    if !args.build_args.config.is_empty() {
        eprintln!("Error: Unsupported option: --config is not yet supported in mitts");
        return 1;
    }

    let mut opt = to_rust_options(&args.build_args);
    // Trailing arguments after -- are for the binary execution, not rustc
    opt.extra_args.clear();

    let manifest_path = match RustAdapter::resolve_manifest(&opt) {
        Ok(p) => p,
        Err(err) => {
            Ui::report_error(&err);
            return 1;
        }
    };

    let selected = match resolve_run_target(&opt, &manifest_path) {
        Ok(s) => s,
        Err(code) => return code,
    };

    opt.package = Some(selected.pkg_name.clone());
    opt.bin_targets = vec![selected.bin_name.clone()];

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

    let _cache_lease = crate::core::gc::CacheLease::acquire(opt.cache_dir.as_deref());
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

            let mut bin_path = None;
            let mut bin_env = HashMap::new();
            for action in &plan.actions {
                if action.kind == ActionKind::CompileBin {
                    for out in &action.outputs {
                        let matches_selected = out
                            .file_name()
                            .and_then(|n| n.to_str())
                            .is_some_and(|n| n.trim_end_matches(".exe") == selected.bin_name);
                        if matches_selected || bin_path.is_none() {
                            bin_path = Some(out.clone());
                            bin_env = action.env.clone();
                            if matches_selected {
                                break;
                            }
                        }
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
            let abs_pkg_dir = if selected.pkg_dir.is_absolute() {
                selected.pkg_dir.clone()
            } else {
                std::env::current_dir()
                    .unwrap_or_else(|_| PathBuf::from("."))
                    .join(&selected.pkg_dir)
            };
            let mut cmd = Command::new(&binary);
            cmd.args(trailing);
            crate::adapter::rust::unit_env::apply_runtime_execution_env(
                &mut cmd,
                &abs_pkg_dir,
                &bin_env,
                &plan,
                opt.release,
            );
            let status = cmd.status();

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
