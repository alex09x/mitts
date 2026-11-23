/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Future-incompatibility reports: `--future-incompat-report` checks the workspace once
//! more with rustc's `--json=future-incompat`, prints what packages will stop compiling in
//! a future Rust, and keeps the report in `target/.future-incompat-report.json` for
//! `mitts report future-incompatibilities`. Ordinary builds are not changed by it.

use anyhow::{bail, Context, Result};
use clap::{Args, Subcommand};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Args, Debug, Clone)]
pub struct ReportArgs {
    #[command(subcommand)]
    pub command: ReportCommand,
}

#[derive(Subcommand, Debug, Clone)]
pub enum ReportCommand {
    /// Reports any crates which will eventually stop compiling
    #[command(visible_alias = "future-incompat")]
    FutureIncompatibilities(FutureArgs),
}

#[derive(Args, Debug, Clone, Default)]
pub struct FutureArgs {
    /// Identifier of the report to display (the newest by default)
    #[arg(long)]
    pub id: Option<u64>,

    /// Package to display a report for
    #[arg(short = 'p', long = "package", value_name = "SPEC")]
    pub package: Option<String>,

    /// Path to Cargo.toml
    #[arg(long)]
    pub manifest_path: Option<PathBuf>,

    /// Do not print Mitts log messages
    #[arg(short, long)]
    pub quiet: bool,

    /// Use verbose output
    #[arg(short, long, action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// Require Cargo.lock is up to date (accepted for Cargo compatibility)
    #[arg(long)]
    pub locked: bool,

    /// Run without accessing the network (accepted for Cargo compatibility)
    #[arg(long)]
    pub offline: bool,

    /// Equivalent to specifying both --locked and --offline
    #[arg(long)]
    pub frozen: bool,
}

fn report_path(manifest: &Option<PathBuf>) -> Result<PathBuf> {
    let start = super::project_info::starting_manifest(manifest)?;
    let root = super::project_info::workspace_root(&start)?;
    let opt = crate::adapter::RustBuildOptions {
        manifest_path: Some(root.clone()),
        ..Default::default()
    };
    let target = crate::adapter::RustAdapter::resolve_target_dir_without_tags(&root, &opt)?;
    Ok(target.join(".future-incompat-report.json"))
}

/// `(package, rendered lints)` from the JSON lines of a check run.
pub fn collect(stdout: &str) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for line in stdout.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let report = if value["$message_type"] == "future_incompat" {
            &value["future_incompat_report"]
        } else if value["message"]["$message_type"] == "future_incompat" {
            &value["message"]["future_incompat_report"]
        } else {
            continue;
        };
        let fallback = value["package_id"].as_str().map(display_package);
        for item in report.as_array().into_iter().flatten() {
            let diagnostic = &item["diagnostic"];
            let rendered = diagnostic["rendered"].as_str().unwrap_or("").to_string();
            let package = diagnostic["spans"]
                .as_array()
                .and_then(|spans| {
                    spans
                        .iter()
                        .find_map(|s| s["file_name"].as_str().and_then(package_of_path))
                })
                .or_else(|| fallback.clone())
                .unwrap_or_else(|| "unknown package".to_string());
            let entry = out.entry(package).or_default();
            if !entry.contains(&rendered) {
                entry.push(rendered);
            }
        }
    }
    out
}

/// `name v1.2.3` from a registry path segment `name-1.2.3`.
fn package_of_path(file: &str) -> Option<String> {
    Path::new(file).components().find_map(|c| {
        let word = c.as_os_str().to_string_lossy();
        let (name, version) = word.rsplit_once('-')?;
        let mut parts = version.split('.');
        let numeric = parts.clone().count() >= 3
            && parts.all(|p| p.chars().next().is_some_and(|ch| ch.is_ascii_digit()));
        numeric.then(|| format!("{name} v{version}"))
    })
}

fn display_package(id: &str) -> String {
    // `name@1.0.0 (source)` or a Cargo package id `source#name@1.0.0`.
    let id = id.rsplit('#').next().unwrap_or(id);
    let id = id.split(" (").next().unwrap_or(id);
    match id.split_once('@') {
        Some((n, v)) => format!("{n} v{v}"),
        None => id.to_string(),
    }
}

fn render(per_package: &BTreeMap<String, Vec<String>>, id: u64) -> String {
    let names: Vec<&String> = per_package.keys().collect();
    let mut text = format!(
        "warning: the following packages contain code that will be rejected by a future version of Rust: {}\n",
        names.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
    );
    text.push_str(&format!(
        "note: \nTo solve this problem, you can try the following approaches:\n\n\
         - If the issue is not solved by updating the dependencies, a fix has to be\n\
         implemented by those dependencies. You can help with that by notifying the\n\
         maintainers of this problem (e.g. by creating a bug report) or by proposing a\n\
         fix to the maintainers (e.g. by creating a pull request).\n\n\
         - If waiting for an upstream fix is not an option, you can use the `[patch]`\n\
         section in `Cargo.toml` to use your own version of the dependency.\n\n\
         note: this report can be shown with `mitts report future-incompatibilities --id {id}`\n"
    ));
    for (package, items) in per_package {
        text.push_str(&format!(
            "\nThe package `{package}` currently triggers the following future incompatibility lints:\n"
        ));
        for item in items {
            for line in item.lines() {
                text.push_str(&format!("> {line}\n"));
            }
        }
    }
    text
}

/// After a build with `--future-incompat-report`: check again with the report enabled,
/// print it and store it.
pub fn run_after_build() {
    if let Err(error) = run_after_build_inner() {
        eprintln!("warning: future-incompatibility report not produced: {error:#}");
    }
}

fn run_after_build_inner() -> Result<()> {
    let exe = std::env::current_exe()?;
    let mut flags = std::env::var("RUSTFLAGS").unwrap_or_default();
    if !flags.is_empty() {
        flags.push(' ');
    }
    flags.push_str("--json=future-incompat");
    let output = Command::new(exe)
        .args([
            "check",
            "--workspace",
            "--all-targets",
            "--message-format",
            "json",
            "-q",
        ])
        .env("RUSTFLAGS", flags)
        .output()
        .context("run mitts check")?;
    let per_package = collect(&String::from_utf8_lossy(&output.stdout));
    if per_package.is_empty() {
        eprintln!("note: 0 dependencies had future-incompatible warnings");
        return Ok(());
    }
    let path = report_path(&None)?;
    let mut doc: Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| json!({"version": 0, "next_id": 1, "reports": []}));
    let id = doc["next_id"].as_u64().unwrap_or(1);
    doc["next_id"] = json!(id + 1);
    let mut reports = doc["reports"].as_array().cloned().unwrap_or_default();
    reports.push(json!({"id": id, "per_package": per_package}));
    // Cargo keeps the last few reports.
    let keep = reports.len().saturating_sub(5);
    doc["reports"] = Value::Array(reports.split_off(keep));
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, serde_json::to_string_pretty(&doc)?)?;
    eprint!("{}", render(&per_package, id));
    Ok(())
}

pub fn handle_report(args: &ReportArgs) -> i32 {
    let ReportCommand::FutureIncompatibilities(args) = &args.command;
    match show(args) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("error: {error:#}");
            101
        }
    }
}

fn show(args: &FutureArgs) -> Result<()> {
    let path = report_path(&args.manifest_path)?;
    let doc: Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .context("no reports are currently available")?;
    let reports = doc["reports"].as_array().cloned().unwrap_or_default();
    let report = match args.id {
        Some(id) => reports
            .iter()
            .find(|r| r["id"].as_u64() == Some(id))
            .with_context(|| {
                let ids: Vec<String> = reports
                    .iter()
                    .filter_map(|r| r["id"].as_u64().map(|i| i.to_string()))
                    .collect();
                format!(
                    "could not find report with ID {id}\nAvailable IDs are: {}",
                    ids.join(", ")
                )
            })?,
        None => reports
            .last()
            .context("no reports are currently available")?,
    };
    let mut per_package: BTreeMap<String, Vec<String>> =
        serde_json::from_value(report["per_package"].clone())?;
    if let Some(spec) = &args.package {
        let (name, _) = super::project_info::parse_spec(spec);
        per_package.retain(|k, _| k.split(' ').next() == Some(name));
        if per_package.is_empty() {
            bail!("could not find package with ID `{spec}`");
        }
    }
    print!(
        "{}",
        render(&per_package, report["id"].as_u64().unwrap_or(0))
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_are_grouped_by_package() {
        let line = json!({
            "$message_type": "future_incompat",
            "future_incompat_report": [{"diagnostic": {
                "rendered": "warning: old syntax\n",
                "spans": [{"file_name": "/home/u/.cache/mitts/sources/oldcrate-0.1.2/src/lib.rs"}]
            }}]
        })
        .to_string();
        let found = collect(&line);
        assert_eq!(found["oldcrate v0.1.2"], vec!["warning: old syntax\n"]);
        let text = render(&found, 3);
        assert!(text.starts_with("warning: the following packages contain code that will be rejected by a future version of Rust: oldcrate v0.1.2\n"));
        assert!(text.contains("> warning: old syntax\n"));
        assert!(text.contains("--id 3"));
    }
}
