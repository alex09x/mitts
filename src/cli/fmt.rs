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
use crate::adapter::{RustAdapter, RustBuildOptions};
use crate::cli::Ui;
use clap::Args;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Args, Debug, Clone, Default)]
pub struct FmtArgs {
    /// Path to Cargo.toml
    #[arg(long)]
    pub manifest_path: Option<PathBuf>,

    /// Package to format
    #[arg(short, long)]
    pub package: Option<String>,

    /// Run in 'check' mode to exit with an error if files are unformatted
    #[arg(long)]
    pub check: bool,

    /// Format all packages in the workspace
    #[arg(long)]
    pub all: bool,

    /// Use verbose output
    #[arg(short, long, action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// Options passed to rustfmt
    #[arg(last = true)]
    pub extra_args: Vec<String>,
}

fn resolve_rustfmt() -> PathBuf {
    if let Ok(p) = std::env::var("RUSTFMT") {
        return PathBuf::from(p);
    }
    if let Ok(home) = std::env::var("HOME") {
        let p = PathBuf::from(home)
            .join(".cargo")
            .join("bin")
            .join("rustfmt");
        if p.is_file() {
            return p;
        }
    }
    PathBuf::from("rustfmt")
}

fn collect_package_source_files(pkg_dir: &Path, files: &mut Vec<PathBuf>) {
    let subdirs = ["src", "tests", "examples", "benches"];
    for subdir in subdirs {
        let dir = pkg_dir.join(subdir);
        if dir.is_dir() {
            collect_rs_files_recursively(&dir, files);
        }
    }
    let build_rs = pkg_dir.join("build.rs");
    if build_rs.is_file() {
        files.push(build_rs);
    }
}

fn collect_rs_files_recursively(dir: &Path, files: &mut Vec<PathBuf>) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if name != "target" && name != ".mitts" && name != ".git" {
                    collect_rs_files_recursively(&p, files);
                }
            } else if p.is_file() && p.extension().and_then(|e| e.to_str()) == Some("rs") {
                files.push(p);
            }
        }
    }
}

pub fn handle_fmt(args: &FmtArgs) -> i32 {
    let start = std::time::Instant::now();
    let opt = RustBuildOptions {
        manifest_path: args.manifest_path.clone(),
        package: args.package.clone(),
        verbose: args.verbose,
        ..Default::default()
    };

    let manifest_path = match RustAdapter::resolve_manifest(&opt) {
        Ok(p) => p,
        Err(err) => {
            Ui::report_error(&err);
            return 1;
        }
    };

    let project_dir = manifest_path.parent().unwrap_or(Path::new("."));
    let root_toml = match CargoToml::from_file(&manifest_path) {
        Ok(t) => t,
        Err(err) => {
            Ui::report_error(&err);
            return 1;
        }
    };

    let members = root_toml.discover_workspace_members(project_dir);
    let mut edition_groups: HashMap<String, Vec<PathBuf>> = HashMap::new();

    for (name, member_dir) in &members {
        if let Some(ref target_pkg) = args.package {
            if name != target_pkg {
                continue;
            }
        }
        let m_path = member_dir.join("Cargo.toml");
        let pkg_edition = if let Ok(toml) = CargoToml::from_file(&m_path) {
            toml.package
                .and_then(|p| p.edition)
                .unwrap_or_else(|| "2021".to_string())
        } else {
            "2021".to_string()
        };
        let mut pkg_files = Vec::new();
        collect_package_source_files(member_dir, &mut pkg_files);
        edition_groups
            .entry(pkg_edition)
            .or_default()
            .extend(pkg_files);
    }

    let mut total_files = 0;
    for files in edition_groups.values_mut() {
        files.sort();
        files.dedup();
        total_files += files.len();
    }

    if total_files == 0 {
        println!("mitts fmt: no source files found to format");
        return 0;
    }

    let rustfmt_bin = resolve_rustfmt();
    let mode_str = if args.check { "check" } else { "format" };
    Ui::header(&format!("fmt [{}]", mode_str));

    let mut editions_desc: Vec<String> = edition_groups
        .iter()
        .map(|(ed, fls)| format!("{} files (edition {})", fls.len(), ed))
        .collect();
    editions_desc.sort();
    Ui::stage_start(
        "🎨",
        &format!("Running rustfmt over {}", editions_desc.join(", ")),
    );

    for (edition, files) in &edition_groups {
        let mut cmd = Command::new(&rustfmt_bin);
        cmd.arg("--edition").arg(edition);
        if args.check {
            cmd.arg("--check");
        }
        if args.verbose > 0 {
            cmd.arg("-v");
        }
        for extra in &args.extra_args {
            cmd.arg(extra);
        }
        for file in files {
            cmd.arg(file);
        }

        let status = match cmd.status() {
            Ok(s) => s,
            Err(err) => {
                eprintln!("mitts error: failed to invoke rustfmt: {}", err);
                return 1;
            }
        };

        if !status.success() {
            let dur = format!("{:.2}s", start.elapsed().as_secs_f64());
            Ui::finish_failed(
                "fmt",
                &dur,
                "Formatting differences detected or rustfmt failed",
            );
            return status.code().unwrap_or(1);
        }
    }

    let dur = format!("{:.2}s", start.elapsed().as_secs_f64());
    let summary = if args.check {
        "Formatting check passed (0 diffs)"
    } else {
        "Formatted files successfully"
    };
    Ui::stage_item(true, &Ui::badge_cached(), summary);
    Ui::finish("fmt", &dur, summary);
    0
}
