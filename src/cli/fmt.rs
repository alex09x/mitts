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

fn collect_named_dir_roots(
    pkg_dir: &Path,
    subdir: &str,
    explicit: &[crate::adapter::rust::manifest_models::TomlTest],
    auto_enabled: bool,
    files: &mut Vec<PathBuf>,
) {
    let mut explicit_paths = std::collections::HashSet::new();
    for item in explicit {
        let p = match item.path {
            Some(ref rel) => pkg_dir.join(rel),
            None => pkg_dir.join(subdir).join(format!("{}.rs", item.name)),
        };
        if p.is_file() {
            explicit_paths.insert(p.clone());
            files.push(p);
        }
    }
    if !auto_enabled {
        return;
    }
    let dir = pkg_dir.join(subdir);
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_file()
                && p.extension().and_then(|e| e.to_str()) == Some("rs")
                && !explicit_paths.contains(&p)
            {
                if p.file_stem()
                    .and_then(|s| s.to_str())
                    .is_some_and(|s| !s.starts_with('.'))
                {
                    files.push(p);
                }
            } else if p.is_dir() {
                let main_rs = p.join("main.rs");
                if main_rs.is_file()
                    && !explicit_paths.contains(&main_rs)
                    && p.file_name()
                        .and_then(|s| s.to_str())
                        .is_some_and(|s| !s.starts_with('.'))
                {
                    files.push(main_rs);
                }
            }
        }
    }
}

fn collect_package_source_files(pkg_dir: &Path, toml: &CargoToml, files: &mut Vec<PathBuf>) {
    for t in toml.discover_targets(pkg_dir) {
        if t.src_path.is_file() {
            files.push(t.src_path);
        }
    }
    for t in toml.discover_test_targets(pkg_dir) {
        if t.src_path.is_file() {
            files.push(t.src_path);
        }
    }
    let autoexamples = toml
        .package
        .as_ref()
        .and_then(|p| p.autoexamples)
        .unwrap_or(true);
    collect_named_dir_roots(pkg_dir, "examples", &toml.example, autoexamples, files);
    let autobenches = toml
        .package
        .as_ref()
        .and_then(|p| p.autobenches)
        .unwrap_or(true);
    collect_named_dir_roots(pkg_dir, "benches", &toml.bench, autobenches, files);

    if toml.has_build_script(pkg_dir) {
        let build_path = match toml.package.as_ref().and_then(|p| p.build.as_ref()) {
            Some(crate::adapter::rust::manifest_models::TomlBuild::Path(ref p)) => pkg_dir.join(p),
            _ => pkg_dir.join("build.rs"),
        };
        if build_path.is_file() {
            files.push(build_path);
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

    let mut members = match root_toml.discover_workspace_members(project_dir) {
        Ok(m) => m,
        Err(err) => {
            Ui::report_error(&err);
            return 1;
        }
    };
    if args.all {
        let mut extra_dirs = Vec::new();
        let mut scan_deps =
            |base: &Path,
             deps: &HashMap<String, crate::adapter::rust::manifest::TomlDependency>| {
                for dep in deps.values() {
                    if let crate::adapter::rust::manifest::TomlDependency::Detailed(ref d) = dep {
                        if let Some(ref rel) = d.path {
                            let dep_dir = base.join(rel);
                            if dep_dir.join("Cargo.toml").is_file() {
                                extra_dirs.push(dep_dir);
                            }
                        }
                    }
                }
            };
        if let Some(ref ws) = root_toml.workspace {
            scan_deps(project_dir, &ws.dependencies);
        }
        for (_, m_dir) in &members {
            if let Ok(t) = CargoToml::from_file(&m_dir.join("Cargo.toml")) {
                scan_deps(m_dir, &t.dependencies);
                scan_deps(m_dir, &t.dev_dependencies);
                scan_deps(m_dir, &t.build_dependencies);
            }
        }
        for dep_dir in extra_dirs {
            if let Ok(t) = CargoToml::from_file(&dep_dir.join("Cargo.toml")) {
                if let Some(pkg) = t.package {
                    if !members.iter().any(|(n, _)| n == &pkg.name) {
                        members.push((pkg.name, dep_dir));
                    }
                }
            }
        }
    }
    let mut edition_groups: HashMap<String, Vec<PathBuf>> = HashMap::new();

    for (name, member_dir) in &members {
        if let Some(ref target_pkg) = args.package {
            if name != target_pkg {
                continue;
            }
        }
        let m_path = member_dir.join("Cargo.toml");
        let toml = CargoToml::from_file(&m_path).unwrap_or_default();
        let pkg_edition = toml
            .package
            .as_ref()
            .and_then(|p| p.edition.clone())
            .unwrap_or_else(|| "2015".to_string());
        let mut pkg_files = Vec::new();
        collect_package_source_files(member_dir, &toml, &mut pkg_files);
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

    let mut sorted_editions: Vec<_> = edition_groups.iter().collect();
    sorted_editions.sort_by_key(|(ed, _)| *ed);
    for (edition, files) in sorted_editions {
        for chunk in files.chunks(256) {
            let mut cmd = Command::new(&rustfmt_bin);
            cmd.arg("--edition").arg(edition);
            if args.check {
                cmd.arg("--check");
            }
            if args.verbose > 0 {
                cmd.arg("-v");
            }
            cmd.args(&args.extra_args);
            cmd.args(chunk);

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
