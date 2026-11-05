/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::adapter::{BuildMode, RustAdapter, RustBuildOptions};
use crate::cli::ui::Ui;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub use crate::cli::coverage_tools::resolve_llvm_tool;
pub use crate::cli::coverage_types::CoverageArgs;

pub fn handle_coverage(args: &CoverageArgs) -> i32 {
    if !args.config.is_empty() {
        eprintln!("Error: Unsupported option: --config is not yet supported in mitts");
        return 1;
    }
    if args.doc {
        eprintln!("error: --doc is not supported with coverage");
        return 101;
    }
    if args.output_path.is_some() {
        if args.html {
            eprintln!("error: --output-path writes one file; use --output-dir for --html");
            return 2;
        }
        let single_file_formats = [args.json, args.lcov, args.text];
        if single_file_formats.iter().filter(|chosen| **chosen).count() != 1 {
            eprintln!("error: --output-path needs exactly one of --json, --lcov or --text");
            return 2;
        }
    }

    let start = std::time::Instant::now();
    let (llvm_profdata, llvm_cov) = if !args.no_run {
        let profdata = match resolve_llvm_tool("llvm-profdata") {
            Some(p) => p,
            None => {
                eprintln!("mitts coverage error: llvm-profdata not found in rustlib or PATH");
                eprintln!("Install with: rustup component add llvm-tools-preview");
                return 1;
            }
        };
        let cov = match resolve_llvm_tool("llvm-cov") {
            Some(p) => p,
            None => {
                eprintln!("mitts coverage error: llvm-cov not found in rustlib or PATH");
                eprintln!("Install with: rustup component add llvm-tools-preview");
                return 1;
            }
        };
        (Some(profdata), Some(cov))
    } else {
        (None, None)
    };

    let mut extra_args = vec!["-C".to_string(), "instrument-coverage".to_string()];
    if args.workspace {
        extra_args.push("--workspace".to_string());
    }

    let opt = RustBuildOptions {
        manifest_path: args.manifest_path.clone(),
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
        no_cache: args.no_cache,
        cache_dir: args.cache_dir.clone(),
        target_triple: args.target.clone(),
        locked: args.locked,
        offline: args.offline,
        frozen: args.frozen,
        fast_lock: None,
        mode: BuildMode::Test,
        native: true,
        cargo: false,
        extra_args,
        verbose: args.verbose,
        progress: None,
        start_dir: None,
        test_targets: args.test.clone(),
        all_tests: args.tests || args.all_targets,
        all_benches: args.all_targets,
        bench_targets: Vec::new(),
        all_examples: args.all_targets,
        bin_targets: args.bin.clone(),
        all_bins: args.bins || args.all_targets,
        lib_only: args.lib,
        doc_only: args.doc,
        exclude: args.exclude.clone(),
        example_targets: Vec::new(),
        crate_types: Vec::new(),
        quiet: false,
        json_diagnostics: false,
    };

    let manifest_path = match RustAdapter::resolve_manifest(&opt) {
        Ok(m) => m,
        Err(err) => {
            Ui::report_error(&err);
            return 1;
        }
    };
    let root_dir = {
        let p = manifest_path.parent().unwrap_or_else(|| Path::new("."));
        if p.as_os_str().is_empty() {
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
        } else {
            fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
        }
    };

    if let Err(err) =
        crate::adapter::rust::test_selection::validate_test_options(&opt, &manifest_path, &root_dir)
    {
        eprintln!("error: {:#}", err);
        return 101;
    }

    let profile = if args.release { "release" } else { "dev" };
    let data_on_stdout = args.json_report_on_stdout();
    if !data_on_stdout {
        Ui::header(&format!("coverage [{}]", profile));
        Ui::stage_start(
            "📊",
            "Compiling and instrumenting test targets for LLVM coverage",
        );
    }

    let _cache_lease = crate::core::gc::CacheLease::acquire(opt.cache_dir.as_deref());
    let (plan, result) = match RustAdapter::build(&opt) {
        Ok(res) => res,
        Err(err) => {
            Ui::report_error(&err);
            return 1;
        }
    };

    if !result.success {
        eprintln!(
            "mitts coverage error: compilation failed:\n{}",
            result.stderr
        );
        return result.exit_code;
    }

    if args.no_run {
        let dur = format!("{:.2}s", start.elapsed().as_secs_f64());
        Ui::finish("coverage", &dur, "Instrumented test targets compiled");
        return 0;
    }

    let (discovered, _) = match super::test_runner::discover_test_binaries(&plan, &opt, &root_dir) {
        Ok(res) => res,
        Err(err) => {
            eprintln!("mitts coverage error: {:#}", err);
            return 1;
        }
    };

    if discovered.test_binaries.is_empty() {
        eprintln!("mitts coverage: no test binaries found to run");
        return 0;
    }

    let profdata_dir = plan.target_directory.join("coverage");
    let profraw_dir = profdata_dir.join(format!("profraw-{}", std::process::id()));
    let _ = fs::remove_dir_all(&profraw_dir);
    let _ = fs::create_dir_all(&profraw_dir);

    if !data_on_stdout {
        Ui::stage_start(
            "🧪",
            &format!(
                "Running {} test binaries with profile collection",
                discovered.test_binaries.len()
            ),
        );
    }
    let profpattern = profraw_dir.join("cov-%p-%m.profraw");

    let mut failed_exit_code: Option<i32> = None;
    if let Some(requested) = args.concurrency {
        // Every test is a process of its own with its own profile file (`%p` is the process id).
        let selection = super::test::TestArgs {
            filter: args.filter.clone(),
            exact: args.exact,
            ignored: args.ignored,
            no_fail_fast: args.no_fail_fast,
            release: args.release,
            ..Default::default()
        };
        let code = super::test_parallel::run(
            &discovered,
            &selection,
            &root_dir,
            &plan,
            profile,
            start,
            requested,
            &[(
                "LLVM_PROFILE_FILE".to_string(),
                profpattern.clone().into_os_string(),
            )],
        );
        if code != 0 {
            if !args.no_fail_fast {
                let _ = fs::remove_dir_all(&profraw_dir);
                return code;
            }
            failed_exit_code = Some(code);
        }
    }
    // The tests run one binary after another unless they were run in parallel above.
    let serial = if args.concurrency.is_some() {
        &[][..]
    } else {
        &discovered.test_binaries[..]
    };
    for (_, bin_path, env) in serial {
        let abs_bin = if bin_path.is_absolute() {
            bin_path.clone()
        } else {
            root_dir.join(bin_path)
        };
        let mut cmd = Command::new(&abs_bin);
        cmd.env("LLVM_PROFILE_FILE", &profpattern);
        for (k, v) in env.iter() {
            cmd.env(k, v);
        }
        let work_dir = if let Some(manifest_dir) = env.get("CARGO_MANIFEST_DIR") {
            if !manifest_dir.is_empty() && manifest_dir != "." {
                if Path::new(manifest_dir).is_absolute() {
                    PathBuf::from(manifest_dir)
                } else {
                    root_dir.join(manifest_dir)
                }
            } else {
                root_dir.clone()
            }
        } else {
            root_dir.clone()
        };
        cmd.current_dir(&work_dir);

        if !env.contains_key("CARGO_TARGET_TMPDIR") {
            let tmp_dir = plan.target_directory.join("tmp");
            let _ = fs::create_dir_all(&tmp_dir);
            cmd.env("CARGO_TARGET_TMPDIR", tmp_dir);
        }

        crate::adapter::rust::unit_env::apply_runtime_execution_env(
            &mut cmd,
            &work_dir,
            env,
            &plan,
            args.release,
        );

        if let Some(ref filter) = args.filter {
            cmd.arg(filter);
        }
        if args.exact && !args.test_args.iter().any(|a| a == "--exact") {
            cmd.arg("--exact");
        }
        if args.ignored && !args.test_args.iter().any(|a| a == "--ignored") {
            cmd.arg("--ignored");
        }
        if args.nocapture && !args.test_args.iter().any(|a| a == "--nocapture") {
            cmd.arg("--nocapture");
        }
        for extra in &args.test_args {
            cmd.arg(extra);
        }
        if data_on_stdout {
            // The tests' own stdout must not reach the JSON report: send it to stderr.
            if let Some(stdio) = stderr_as_stdio() {
                cmd.stdout(stdio);
            }
        }
        let status = match cmd.status() {
            Ok(s) => s,
            Err(err) => {
                let _ = fs::remove_dir_all(&profraw_dir);
                eprintln!(
                    "mitts coverage error: failed executing {:?}: {}",
                    bin_path, err
                );
                return 1;
            }
        };
        if !status.success() {
            #[cfg(unix)]
            {
                use std::os::unix::process::ExitStatusExt;
                if let Some(sig) = status.signal() {
                    eprintln!(
                        "error: test failed, to rerun pass `--test {}`\n\nCaused by:\n  process didn't exit successfully: `{}` (signal: {})",
                        bin_path.file_name().and_then(|s| s.to_str()).unwrap_or("test"),
                        abs_bin.display(),
                        sig
                    );
                }
            }
            eprintln!("mitts coverage: test failure in {:?}", bin_path);
            let code = status.code().unwrap_or(101);
            if !args.no_fail_fast {
                let _ = fs::remove_dir_all(&profraw_dir);
                return code;
            }
            if failed_exit_code.is_none() {
                failed_exit_code = Some(code);
            }
        }
    }

    let ws_root = crate::adapter::rust::compiler_resolve::find_workspace_root(&manifest_path);
    let report_code = crate::cli::coverage_tools::merge_and_report_coverage(
        args,
        llvm_profdata.as_deref().unwrap(),
        llvm_cov.as_deref().unwrap(),
        &profdata_dir,
        &profraw_dir,
        &ws_root,
        &discovered.test_binaries,
        &discovered.cli_binaries,
        start,
    );
    failed_exit_code.unwrap_or(report_code)
}

/// A duplicate of this process's stderr, usable as a child's stdout.
fn stderr_as_stdio() -> Option<std::process::Stdio> {
    #[cfg(unix)]
    {
        use std::os::fd::AsFd;
        std::io::stderr()
            .as_fd()
            .try_clone_to_owned()
            .ok()
            .map(std::process::Stdio::from)
    }
    #[cfg(not(unix))]
    {
        None
    }
}
