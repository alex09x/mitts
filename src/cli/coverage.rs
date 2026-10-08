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
use clap::Args;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Args, Debug, Clone, Default)]
pub struct CoverageArgs {
    /// Path to Cargo.toml
    #[arg(long)]
    pub manifest_path: Option<PathBuf>,

    /// Package to measure coverage for
    #[arg(short, long)]
    pub package: Option<String>,

    /// Build and run tests in release mode
    #[arg(short, long)]
    pub release: bool,

    /// Only print high-level coverage summary table
    #[arg(long)]
    pub summary_only: bool,

    /// Generate HTML coverage report
    #[arg(long)]
    pub html: bool,

    /// Generate LCOV coverage report (lcov.info)
    #[arg(long)]
    pub lcov: bool,

    /// Generate JSON coverage report
    #[arg(long)]
    pub json: bool,

    /// Generate plain text coverage report
    #[arg(long)]
    pub text: bool,

    /// Output directory for HTML or LCOV reports
    #[arg(long)]
    pub output_dir: Option<PathBuf>,

    /// Automatically open the generated HTML report
    #[arg(long)]
    pub open: bool,

    /// Minimum line coverage percentage (fails if below threshold)
    #[arg(long)]
    pub fail_under_lines: Option<f64>,

    /// Filter tests by name or substring
    #[arg(value_name = "TESTNAME")]
    pub filter: Option<String>,

    /// Build all targets in the workspace
    #[arg(long)]
    pub workspace: bool,

    /// Build all targets (bins, tests, benches)
    #[arg(long)]
    pub all_targets: bool,

    /// Directory for all generated artifacts
    #[arg(long)]
    pub target_dir: Option<PathBuf>,

    /// Space or comma separated list of features to activate
    #[arg(short = 'F', long)]
    pub features: Vec<String>,

    /// Do not activate the `default` feature
    #[arg(long)]
    pub no_default_features: bool,

    /// Activate all available features
    #[arg(long)]
    pub all_features: bool,

    /// Do not read from or write to the shared action cache
    #[arg(long)]
    pub no_cache: bool,

    /// Directory for the shared artifact and action cache
    #[arg(long)]
    pub cache_dir: Option<PathBuf>,

    /// Use verbose output
    #[arg(short, long, action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// Arguments passed to the test binary runner
    #[arg(last = true)]
    pub test_args: Vec<String>,
}

pub use crate::cli::coverage_tools::resolve_llvm_tool;

pub fn handle_coverage(args: &CoverageArgs) -> i32 {
    let start = std::time::Instant::now();
    let llvm_profdata = match resolve_llvm_tool("llvm-profdata") {
        Some(p) => p,
        None => {
            eprintln!("mitts coverage error: llvm-profdata not found in rustlib or PATH");
            eprintln!("Install with: rustup component add llvm-tools-preview");
            return 1;
        }
    };
    let llvm_cov = match resolve_llvm_tool("llvm-cov") {
        Some(p) => p,
        None => {
            eprintln!("mitts coverage error: llvm-cov not found in rustlib or PATH");
            eprintln!("Install with: rustup component add llvm-tools-preview");
            return 1;
        }
    };

    let profile = if args.release { "release" } else { "dev" };
    Ui::header(&format!("coverage [{}]", profile));
    Ui::stage_start(
        "📊",
        "Compiling and instrumenting test targets for LLVM coverage",
    );

    let opt = RustBuildOptions {
        manifest_path: args.manifest_path.clone(),
        package: args.package.clone(),
        release: args.release,
        target_dir: args.target_dir.clone(),
        features: args.features.clone(),
        no_default_features: args.no_default_features,
        all_features: args.all_features,
        no_cache: args.no_cache,
        cache_dir: args.cache_dir.clone(),
        mode: BuildMode::Test,
        native: true,
        cargo: false,
        extra_args: vec!["-C".to_string(), "instrument-coverage".to_string()],
        verbose: args.verbose,
        progress: None,
    };

    let manifest_path = match RustAdapter::resolve_manifest(&opt) {
        Ok(m) => m,
        Err(err) => {
            Ui::report_error(&err);
            return 1;
        }
    };
    let project_dir = manifest_path.parent().unwrap_or(Path::new("."));
    let target_dir = RustAdapter::resolve_target_dir(&manifest_path, &opt)
        .unwrap_or_else(|_| project_dir.join("target").join("mitts"));
    let profdata_dir = target_dir.join("coverage");
    let profraw_dir = profdata_dir.join("profraw");
    let _ = fs::remove_dir_all(&profraw_dir);
    let _ = fs::create_dir_all(&profraw_dir);

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

    let mut test_binaries = Vec::new();
    for action in &plan.actions {
        if action.kind == crate::core::ActionKind::CompileBin && action.id.contains("::test:") {
            for out in &action.outputs {
                if out.is_file() {
                    let ext = out.extension().and_then(|e| e.to_str()).unwrap_or("");
                    if ext != "d" && ext != "rmeta" && ext != "rlib" && ext != "pdb" {
                        test_binaries.push(out.clone());
                    }
                }
            }
        }
    }

    let profile_name = if opt.release { "release" } else { "debug" };
    let profile_dir = plan.target_directory.join(profile_name);
    if test_binaries.is_empty() && profile_dir.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&profile_dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_file() {
                    let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
                    if ext == "d" || ext == "rmeta" || ext == "rlib" || ext == "pdb" {
                        continue;
                    }
                    let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    if name.contains("test") {
                        test_binaries.push(p);
                    }
                }
            }
        }
    }

    test_binaries.sort();
    test_binaries.dedup();

    if test_binaries.is_empty() {
        println!("mitts coverage: no test binaries found to run");
        return 0;
    }

    Ui::stage_start(
        "🧪",
        &format!(
            "Running {} test binaries with profile collection",
            test_binaries.len()
        ),
    );
    let profpattern = profraw_dir.join("cov-%p-%m.profraw");

    for bin_path in &test_binaries {
        let mut cmd = Command::new(bin_path);
        cmd.env("LLVM_PROFILE_FILE", &profpattern);
        if let Some(ref filter) = args.filter {
            cmd.arg(filter);
        }
        for extra in &args.test_args {
            cmd.arg(extra);
        }
        let status = match cmd.status() {
            Ok(s) => s,
            Err(err) => {
                eprintln!(
                    "mitts coverage error: failed executing {:?}: {}",
                    bin_path, err
                );
                return 1;
            }
        };
        if !status.success() {
            eprintln!("mitts coverage: test failure in {:?}", bin_path);
            return status.code().unwrap_or(1);
        }
    }

    // Merge .profraw files into .profdata
    let merged_profdata = profdata_dir.join("mitts.profdata");
    let mut merge_cmd = Command::new(&llvm_profdata);
    merge_cmd.arg("merge").arg("-sparse");

    let mut raw_count = 0;
    if let Ok(entries) = fs::read_dir(&profraw_dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.extension().and_then(|e| e.to_str()) == Some("profraw") {
                merge_cmd.arg(&p);
                raw_count += 1;
            }
        }
    }

    if raw_count == 0 {
        eprintln!("mitts coverage error: no .profraw profile files generated during test run");
        return 1;
    }

    merge_cmd.arg("-o").arg(&merged_profdata);
    let merge_status = merge_cmd
        .status()
        .map_err(|e| anyhow::anyhow!("merge failed: {}", e));
    if merge_status.is_err() || !merge_status.unwrap().success() {
        eprintln!("mitts coverage error: failed to merge raw profiles with llvm-profdata");
        return 1;
    }

    // Generate coverage report via llvm-cov
    let mut cov_cmd = Command::new(&llvm_cov);
    let out_dir = args
        .output_dir
        .clone()
        .unwrap_or_else(|| profdata_dir.join("report"));

    if args.html {
        let _ = fs::create_dir_all(&out_dir);
        cov_cmd.arg("show");
        cov_cmd.arg(format!("--instr-profile={}", merged_profdata.display()));
        cov_cmd.arg(format!("--output-dir={}", out_dir.display()));
        cov_cmd.arg("--format=html");
        cov_cmd.arg("--show-line-counts-or-regions");
    } else if args.lcov {
        let lcov_path = out_dir.join("lcov.info");
        if let Some(parent) = lcov_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        cov_cmd.arg("export");
        cov_cmd.arg(format!("--instr-profile={}", merged_profdata.display()));
        cov_cmd.arg("--format=lcov");
    } else if args.json {
        cov_cmd.arg("export");
        cov_cmd.arg(format!("--instr-profile={}", merged_profdata.display()));
    } else {
        cov_cmd.arg("report");
        cov_cmd.arg(format!("--instr-profile={}", merged_profdata.display()));
        cov_cmd.arg("--use-color");
    }

    cov_cmd.arg(&test_binaries[0]);
    for extra_bin in &test_binaries[1..] {
        cov_cmd.arg(format!("--object={}", extra_bin.display()));
    }
    cov_cmd.arg(r"--ignore-filename-regex=(/.cargo/|/rustc/|/target/|\.mitts/)");

    let cov_status = if args.lcov {
        let lcov_path = out_dir.join("lcov.info");
        match cov_cmd.output() {
            Ok(out) => {
                if out.status.success() {
                    let _ = fs::write(&lcov_path, &out.stdout);
                }
                out.status
            }
            Err(err) => {
                eprintln!("mitts coverage error: failed to run llvm-cov: {}", err);
                return 1;
            }
        }
    } else {
        println!();
        match cov_cmd.status() {
            Ok(s) => s,
            Err(err) => {
                eprintln!("mitts coverage error: failed to run llvm-cov: {}", err);
                return 1;
            }
        }
    };

    if let Some(threshold) = args.fail_under_lines {
        let mut check_cmd = Command::new(&llvm_cov);
        check_cmd.arg("export").arg("--summary-only");
        check_cmd.arg(format!("--instr-profile={}", merged_profdata.display()));
        check_cmd.arg(&test_binaries[0]);
        for extra_bin in &test_binaries[1..] {
            check_cmd.arg(format!("--object={}", extra_bin.display()));
        }
        check_cmd.arg(r"--ignore-filename-regex=(/.cargo/|/rustc/|/target/|\.mitts/)");
        if let Ok(check_out) = check_cmd.output() {
            if check_out.status.success() {
                if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&check_out.stdout) {
                    let pct = v
                        .get("data")
                        .and_then(|d| d.get(0))
                        .and_then(|d0| d0.get("totals"))
                        .and_then(|tot| tot.get("lines"))
                        .and_then(|lines| lines.get("percent"))
                        .and_then(|p| p.as_f64())
                        .unwrap_or(0.0);
                    if pct < threshold {
                        eprintln!(
                            "mitts coverage error: line coverage {:.2}% is below threshold {:.2}%",
                            pct, threshold
                        );
                        return 1;
                    }
                    println!(
                        "mitts coverage: line coverage {:.2}% meets threshold {:.2}%",
                        pct, threshold
                    );
                }
            }
        }
    }

    let dur = format!("{:.2}s", start.elapsed().as_secs_f64());
    if args.html {
        Ui::stage_item(
            true,
            &Ui::badge_cached(),
            &format!("HTML report saved to {}", out_dir.display()),
        );
        if args.open {
            let index = out_dir.join("index.html");
            let _ = Command::new("open").arg(&index).status();
        }
    }
    Ui::finish("coverage", &dur, "Coverage report completed");

    cov_status.code().unwrap_or(0)
}
