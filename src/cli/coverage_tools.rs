/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::cli::coverage_types::CoverageArgs;
use crate::cli::ui::Ui;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Resolves standard LLVM tools (llvm-profdata, llvm-cov) from the rustlib sysroot or PATH.
pub fn resolve_llvm_tool(tool_name: &str) -> Option<PathBuf> {
    let env_var = match tool_name {
        "llvm-profdata" => "MITTS_LLVM_PROFDATA",
        "llvm-cov" => "MITTS_LLVM_COV",
        _ => "",
    };
    if !env_var.is_empty() {
        if let Ok(val) = std::env::var(env_var) {
            if val == "missing" || val == "disabled" || val == "none" {
                return None;
            }
            if !val.is_empty() {
                let p = PathBuf::from(val);
                if p.is_file() {
                    return Some(p);
                }
                return None;
            }
        }
    }
    if let Ok(output) = Command::new("rustc").arg("--print").arg("sysroot").output() {
        if output.status.success() {
            let sysroot = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if let Ok(host_out) = Command::new("rustc").arg("-vV").output() {
                let host_str = String::from_utf8_lossy(&host_out.stdout);
                for line in host_str.lines() {
                    if let Some(host) = line.strip_prefix("host: ") {
                        let tool_path = PathBuf::from(&sysroot)
                            .join("lib")
                            .join("rustlib")
                            .join(host.trim())
                            .join("bin")
                            .join(tool_name);
                        if tool_path.is_file() {
                            return Some(tool_path);
                        }
                    }
                }
            }
        }
    }
    if let Some(paths) = std::env::var_os("PATH") {
        for path in std::env::split_paths(&paths) {
            let candidate = path.join(tool_name);
            if candidate.is_file() {
                return Some(candidate);
            }
            #[cfg(windows)]
            {
                let candidate_exe = path.join(format!("{}.exe", tool_name));
                if candidate_exe.is_file() {
                    return Some(candidate_exe);
                }
            }
        }
    }
    None
}

fn collect_object_binaries(
    test_binaries: &[(String, PathBuf, HashMap<String, String>)],
    cli_binaries: &HashMap<String, PathBuf>,
) -> Vec<PathBuf> {
    let mut objects = Vec::new();
    let mut seen = HashSet::new();
    for (_, path, _) in test_binaries {
        let key = fs::canonicalize(path).unwrap_or_else(|_| path.clone());
        if seen.insert(key) {
            objects.push(path.clone());
        }
    }
    let mut cli_keys: Vec<_> = cli_binaries.keys().collect();
    cli_keys.sort();
    for k in cli_keys {
        let path = &cli_binaries[k];
        if path.is_file() {
            let key = fs::canonicalize(path).unwrap_or_else(|_| path.clone());
            if seen.insert(key) {
                objects.push(path.clone());
            }
        }
    }
    objects
}

fn coverage_exclusions(args: &CoverageArgs, ws_root: &Path) -> String {
    let cache = crate::core::cache_root::resolve(args.cache_dir.as_deref());
    let cache = if cache.is_absolute() {
        cache
    } else {
        ws_root.join(cache)
    };
    // A path through a symlink (macOS's /var is /private/var) is recorded in a profile as it was
    // given to the compiler, so the cache is excluded under both spellings.
    let canonical_cache = fs::canonicalize(&cache).unwrap_or_else(|_| cache.clone());
    let mut cache_spellings = vec![canonical_cache];
    if !cache_spellings.contains(&cache) {
        cache_spellings.push(cache);
    }
    let escape = |path: &Path| {
        path.to_string_lossy()
            .chars()
            .flat_map(|c| {
                if ".+*?()|[]{}^$\\".contains(c) {
                    vec!['\\', c]
                } else {
                    vec![c]
                }
            })
            .collect::<String>()
    };
    let mut patterns = vec![
        r"/.cargo/".to_string(),
        r"/rustc/".to_string(),
        // Standard library sources of a rustup toolchain, for example on macOS.
        r"/rustlib/src/".to_string(),
        r"/target/".to_string(),
        r"\.mitts/".to_string(),
    ];
    for cache in &cache_spellings {
        for dir in ["sources", "git"] {
            patterns.push(format!("{}/", escape(&cache.join(dir))));
        }
    }
    if let Some(pattern) = &args.ignore_filename_regex {
        patterns.push(format!("({pattern})"));
    }
    format!("--ignore-filename-regex=({})", patterns.join("|"))
}

#[allow(clippy::too_many_arguments)]
pub fn merge_and_report_coverage(
    args: &CoverageArgs,
    llvm_profdata: &Path,
    llvm_cov: &Path,
    profdata_dir: &Path,
    profraw_dir: &Path,
    ws_root: &Path,
    test_binaries: &[(String, PathBuf, HashMap<String, String>)],
    cli_binaries: &HashMap<String, PathBuf>,
    start: std::time::Instant,
) -> i32 {
    let pid_profdata = profdata_dir.join(format!("mitts-{}.profdata", std::process::id()));
    let merged_profdata = profdata_dir.join("mitts.profdata");
    let mut merge_cmd = Command::new(llvm_profdata);
    merge_cmd.arg("merge").arg("-sparse");

    let mut raw_count = 0;
    if let Ok(entries) = fs::read_dir(profraw_dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.extension().and_then(|e| e.to_str()) == Some("profraw") {
                merge_cmd.arg(&p);
                raw_count += 1;
            }
        }
    }

    if raw_count == 0 {
        let _ = fs::remove_dir_all(profraw_dir);
        eprintln!("mitts coverage error: no .profraw profile files generated during test run");
        return 1;
    }

    merge_cmd.arg("-o").arg(&pid_profdata);
    let merge_status = merge_cmd.status();
    let _ = fs::remove_dir_all(profraw_dir);
    if !merge_status.map(|s| s.success()).unwrap_or(false) {
        let _ = fs::remove_file(&pid_profdata);
        eprintln!("mitts coverage error: failed to merge raw profiles with llvm-profdata");
        return 1;
    }
    let _ = fs::copy(&pid_profdata, &merged_profdata);

    let objects = collect_object_binaries(test_binaries, cli_binaries);
    let mut cov_cmd = Command::new(llvm_cov);
    cov_cmd.current_dir(ws_root);
    let out_dir = args
        .output_dir
        .clone()
        .unwrap_or_else(|| profdata_dir.join("report"));

    // A single-file report (--output-path, or LCOV's lcov.info) is captured and written to a file
    // instead of reaching stdout.
    let report_file: Option<PathBuf> = match (&args.output_path, args.lcov) {
        (Some(path), _) => Some(path.clone()),
        (None, true) => Some(out_dir.join("lcov.info")),
        (None, false) => None,
    };
    if args.html {
        let _ = fs::create_dir_all(&out_dir);
        cov_cmd.arg("show");
        cov_cmd.arg(format!("--instr-profile={}", pid_profdata.display()));
        cov_cmd.arg(format!("--output-dir={}", out_dir.display()));
        cov_cmd.arg("--format=html");
        cov_cmd.arg("--show-line-counts-or-regions");
    } else if args.lcov {
        cov_cmd.arg("export");
        cov_cmd.arg(format!("--instr-profile={}", pid_profdata.display()));
        cov_cmd.arg("--format=lcov");
    } else if args.json {
        cov_cmd.arg("export");
        cov_cmd.arg(format!("--instr-profile={}", pid_profdata.display()));
        if args.summary_only {
            cov_cmd.arg("--summary-only");
        }
    } else {
        cov_cmd.arg("report");
        cov_cmd.arg(format!("--instr-profile={}", pid_profdata.display()));
        if report_file.is_none() {
            cov_cmd.arg("--use-color");
        }
    }

    cov_cmd.arg(&objects[0]);
    for extra_bin in &objects[1..] {
        cov_cmd.arg(format!("--object={}", extra_bin.display()));
    }
    let exclusions = coverage_exclusions(args, ws_root);
    cov_cmd.arg(&exclusions);

    let cov_status = if let Some(path) = &report_file {
        match cov_cmd.output() {
            Ok(out) => {
                if out.status.success() {
                    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                        let _ = fs::create_dir_all(parent);
                    }
                    if let Err(err) = fs::write(path, &out.stdout) {
                        let _ = fs::remove_file(&pid_profdata);
                        eprintln!(
                            "mitts coverage error: cannot write report to {}: {err}",
                            path.display()
                        );
                        return 1;
                    }
                } else {
                    eprint!("{}", String::from_utf8_lossy(&out.stderr));
                }
                out.status
            }
            Err(err) => {
                let _ = fs::remove_file(&pid_profdata);
                eprintln!("mitts coverage error: failed to run llvm-cov: {}", err);
                return 1;
            }
        }
    } else {
        if !args.json_report_on_stdout() && !args.quiet {
            println!();
        }
        match cov_cmd.status() {
            Ok(s) => s,
            Err(err) => {
                let _ = fs::remove_file(&pid_profdata);
                eprintln!("mitts coverage error: failed to run llvm-cov: {}", err);
                return 1;
            }
        }
    };

    if !cov_status.success() {
        let _ = fs::remove_file(&pid_profdata);
        eprintln!("mitts coverage error: llvm-cov report failed");
        return cov_status.code().unwrap_or(1);
    }

    if let Some(threshold) = args.fail_under_lines {
        let mut check_cmd = Command::new(llvm_cov);
        check_cmd.current_dir(ws_root);
        check_cmd.arg("export").arg("--summary-only");
        check_cmd.arg(format!("--instr-profile={}", pid_profdata.display()));
        check_cmd.arg(&objects[0]);
        for extra_bin in &objects[1..] {
            check_cmd.arg(format!("--object={}", extra_bin.display()));
        }
        check_cmd.arg(&exclusions);
        let percent = check_cmd
            .output()
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| serde_json::from_slice::<serde_json::Value>(&output.stdout).ok())
            .and_then(|v| {
                v.get("data")
                    .and_then(|d| d.get(0))
                    .and_then(|d0| d0.get("totals"))
                    .and_then(|tot| tot.get("lines"))
                    .and_then(|lines| lines.get("percent"))
                    .and_then(|p| p.as_f64())
            });
        if let Some(pct) = percent {
            if pct < threshold {
                let _ = fs::remove_file(&pid_profdata);
                eprintln!(
                    "mitts coverage error: line coverage {:.2}% is below threshold {:.2}%",
                    pct, threshold
                );
                return 1;
            }
            let message = format!(
                "mitts coverage: line coverage {:.2}% meets threshold {:.2}%",
                pct, threshold
            );
            if args.json_report_on_stdout() {
                eprintln!("{message}");
            } else {
                println!("{message}");
            }
        } else {
            let _ = fs::remove_file(&pid_profdata);
            eprintln!("mitts coverage error: unable to calculate line coverage threshold");
            return 1;
        }
    }

    let _ = fs::remove_file(&pid_profdata);
    let dur = format!("{:.2}s", start.elapsed().as_secs_f64());
    if !args.quiet {
        if let (Some(path), Some(_)) = (&report_file, &args.output_path) {
            Ui::stage_item(
                true,
                &Ui::badge_cached(),
                &format!("Report saved to {}", path.display()),
            );
        }
        if args.html {
            Ui::stage_item(
                true,
                &Ui::badge_cached(),
                &format!("HTML report saved to {}", out_dir.display()),
            );
        }
    }
    if args.html && args.open {
        let index = out_dir.join("index.html");
        let _ = Command::new("open").arg(&index).status();
    }
    if !args.json_report_on_stdout() && !args.quiet {
        Ui::finish("coverage", &dur, "Coverage report completed");
    }

    0
}
