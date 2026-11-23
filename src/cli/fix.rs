/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! `mitts fix`: checks the selected targets with JSON diagnostics, applies every
//! machine-applicable suggestion in the workspace's own files, and repeats until nothing
//! changes, as `cargo fix` does. The working tree must be clean in git unless allowed.

use anyhow::{bail, Context, Result};
use clap::Args;
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Args, Debug, Clone, Default)]
pub struct FixArgs {
    /// Fix in local packages even if they have errors
    #[arg(long)]
    pub broken_code: bool,

    /// Fix edition migration lints (moving to the next edition)
    #[arg(long)]
    pub edition: bool,

    /// Fix the edition's idiom lints
    #[arg(long)]
    pub edition_idioms: bool,

    /// Fix code even if a VCS was not detected
    #[arg(long)]
    pub allow_no_vcs: bool,

    /// Fix code even if the working directory is dirty or has staged changes
    #[arg(long)]
    pub allow_dirty: bool,

    /// Fix code even if the working directory has staged changes
    #[arg(long)]
    pub allow_staged: bool,

    /// Package(s) to fix
    #[arg(short = 'p', long = "package", value_name = "SPEC", action = clap::ArgAction::Append)]
    pub packages: Vec<String>,

    /// Fix all packages in the workspace
    #[arg(long, visible_alias = "all")]
    pub workspace: bool,

    /// Exclude packages from the fixes
    #[arg(long, value_name = "SPEC", action = clap::ArgAction::Append)]
    pub exclude: Vec<String>,

    /// Fix only this package's library
    #[arg(long)]
    pub lib: bool,

    /// Fix only the specified binary
    #[arg(long, value_name = "NAME", action = clap::ArgAction::Append)]
    pub bin: Vec<String>,

    /// Fix all binaries
    #[arg(long)]
    pub bins: bool,

    /// Fix only the specified example
    #[arg(long, value_name = "NAME", action = clap::ArgAction::Append)]
    pub example: Vec<String>,

    /// Fix all examples
    #[arg(long)]
    pub examples: bool,

    /// Fix only the specified test target
    #[arg(long, value_name = "NAME", action = clap::ArgAction::Append)]
    pub test: Vec<String>,

    /// Fix all targets that have `test = true` set
    #[arg(long)]
    pub tests: bool,

    /// Fix only the specified bench target
    #[arg(long, value_name = "NAME", action = clap::ArgAction::Append)]
    pub bench: Vec<String>,

    /// Fix all targets that have `bench = true` set
    #[arg(long)]
    pub benches: bool,

    /// Fix all targets (default)
    #[arg(long)]
    pub all_targets: bool,

    /// Space or comma separated list of features to activate
    #[arg(short = 'F', long, value_delimiter = ',', num_args = 1..)]
    pub features: Vec<String>,

    /// Activate all available features
    #[arg(long)]
    pub all_features: bool,

    /// Do not activate the `default` feature
    #[arg(long)]
    pub no_default_features: bool,

    /// Fix for the target triple
    #[arg(long)]
    pub target: Option<String>,

    /// Fix artifacts in release mode
    #[arg(short, long)]
    pub release: bool,

    /// Fix artifacts with the specified profile
    #[arg(long, value_name = "PROFILE-NAME")]
    pub profile: Option<String>,

    /// Directory for all generated artifacts
    #[arg(long)]
    pub target_dir: Option<PathBuf>,

    /// Number of parallel jobs
    #[arg(short = 'j', long, value_name = "N")]
    pub jobs: Option<u64>,

    /// Path to Cargo.toml
    #[arg(long)]
    pub manifest_path: Option<PathBuf>,

    /// Directory for the shared artifact and action cache
    #[arg(long)]
    pub cache_dir: Option<PathBuf>,

    /// Require Cargo.lock is up to date
    #[arg(long)]
    pub locked: bool,

    /// Run without accessing the network
    #[arg(long)]
    pub offline: bool,

    /// Equivalent to specifying both --locked and --offline
    #[arg(long)]
    pub frozen: bool,

    /// Do not print Mitts log messages
    #[arg(short, long)]
    pub quiet: bool,

    /// Use verbose output
    #[arg(short, long, action = clap::ArgAction::Count)]
    pub verbose: u8,
}

const MAX_PASSES: usize = 4;

pub fn handle_fix(args: &FixArgs) -> i32 {
    match run(args) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("error: {error:#}");
            101
        }
    }
}

fn status(word: &str, message: &str) {
    eprintln!("{word:>12} {message}");
}

fn run(args: &FixArgs) -> Result<i32> {
    let manifest = super::project_info::starting_manifest(&args.manifest_path)?;
    let root = super::project_info::workspace_root(&manifest)?;
    let ws_dir = root.parent().unwrap_or(Path::new(".")).to_path_buf();
    check_vcs(&ws_dir, args)?;
    let mut lint_flags: Vec<String> = Vec::new();
    if args.edition || args.edition_idioms {
        let edition = package_edition(&manifest);
        if args.edition {
            let next = match edition.as_str() {
                "2015" => "2018",
                "2018" => "2021",
                "2021" => "2024",
                other => bail!("the edition {other} has no next edition to migrate to"),
            };
            if !args.quiet {
                status("Migrating", &format!("{} from {edition} edition to {next}", manifest.display()));
            }
            lint_flags.push(format!("--force-warn=rust-{next}-compatibility"));
        }
        if args.edition_idioms {
            lint_flags.push(format!("--force-warn=rust-{edition}-idioms"));
        }
    }
    let ws_canonical = ws_dir.canonicalize().unwrap_or(ws_dir.clone());
    let mut total: BTreeMap<PathBuf, usize> = BTreeMap::new();
    for _ in 0..MAX_PASSES {
        let output = run_check(args, &lint_flags, true)?;
        let (fixes, has_errors) = suggestions(&output, &ws_canonical);
        if has_errors && !args.broken_code && fixes.is_empty() {
            break;
        }
        if fixes.is_empty() {
            break;
        }
        let applied = apply(&fixes)?;
        if applied.is_empty() {
            break;
        }
        for (file, n) in applied {
            *total.entry(file).or_default() += n;
        }
    }
    if !args.quiet {
        for (file, n) in &total {
            let shown = file.strip_prefix(&ws_canonical).unwrap_or(file);
            let plural = if *n == 1 { "fix" } else { "fixes" };
            status("Fixed", &format!("{} ({n} {plural})", shown.display()));
        }
    }
    // What is left, shown the usual way.
    let last = run_check(args, &lint_flags, false)?;
    if args.edition && !args.quiet {
        eprintln!(
            "note: update the `edition` field in Cargo.toml to finish the migration \
             (and run `mitts fix --edition` again if it reports more)"
        );
    }
    Ok(if last.status.success() { 0 } else { 101 })
}

fn package_edition(manifest: &Path) -> String {
    crate::adapter::rust::manifest::CargoToml::from_file(manifest)
        .ok()
        .and_then(|t| t.package.and_then(|p| p.edition))
        .unwrap_or_else(|| "2015".to_string())
}

/// Cargo's refusal to edit files with uncommitted changes.
fn check_vcs(dir: &Path, args: &FixArgs) -> Result<()> {
    if args.allow_dirty {
        return Ok(());
    }
    let out = Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=no"])
        .current_dir(dir)
        .output();
    let out = match out {
        Ok(o) if o.status.success() => o,
        _ => {
            if args.allow_no_vcs {
                return Ok(());
            }
            bail!(
                "no VCS found for this package and `mitts fix` can potentially perform \
                 destructive changes; if you'd like to suppress this error pass `--allow-no-vcs`"
            );
        }
    };
    let mut dirty = Vec::new();
    let mut staged = Vec::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        if line.len() < 4 {
            continue;
        }
        let (index, worktree, path) = (&line[0..1], &line[1..2], line[3..].to_string());
        if worktree != " " {
            dirty.push(path);
        } else if index != " " && !args.allow_staged {
            staged.push(path);
        }
    }
    if dirty.is_empty() && staged.is_empty() {
        return Ok(());
    }
    let mut files = String::new();
    for f in &dirty {
        files.push_str(&format!("  * {f} (dirty)\n"));
    }
    for f in &staged {
        files.push_str(&format!("  * {f} (staged)\n"));
    }
    bail!(
        "the working directory of this package has uncommitted changes, and `mitts fix` can \
         potentially perform destructive changes; if you'd like to suppress this error pass \
         `--allow-dirty`, or commit the changes to these files:\n\n{files}"
    )
}

fn run_check(args: &FixArgs, lint_flags: &[String], json: bool) -> Result<std::process::Output> {
    let exe = std::env::current_exe().context("locate the mitts executable")?;
    let mut cmd = Command::new(exe);
    cmd.arg("check");
    if json {
        cmd.args(["--message-format", "json"]);
    }
    let selected = args.lib
        || args.bins
        || args.examples
        || args.tests
        || args.benches
        || !args.bin.is_empty()
        || !args.example.is_empty()
        || !args.test.is_empty()
        || !args.bench.is_empty();
    if args.all_targets || !selected {
        cmd.arg("--all-targets");
    }
    for (flag, on) in [
        ("--lib", args.lib),
        ("--bins", args.bins),
        ("--examples", args.examples),
        ("--tests", args.tests),
        ("--benches", args.benches),
        ("--workspace", args.workspace),
        ("--all-features", args.all_features),
        ("--no-default-features", args.no_default_features),
        ("--release", args.release),
        ("--locked", args.locked),
        ("--offline", args.offline),
        ("--frozen", args.frozen),
        ("--quiet", args.quiet || json),
    ] {
        if on {
            cmd.arg(flag);
        }
    }
    for (flag, values) in [
        ("--package", &args.packages),
        ("--exclude", &args.exclude),
        ("--bin", &args.bin),
        ("--example", &args.example),
        ("--test", &args.test),
        ("--bench", &args.bench),
    ] {
        for v in values {
            cmd.args([flag, v]);
        }
    }
    if !args.features.is_empty() {
        cmd.args(["--features", &args.features.join(",")]);
    }
    for (flag, value) in [
        ("--target", args.target.as_ref().map(|t| t.to_string())),
        ("--profile", args.profile.clone()),
        ("--jobs", args.jobs.map(|j| j.to_string())),
        ("--manifest-path", args.manifest_path.as_ref().map(|p| p.display().to_string())),
        ("--cache-dir", args.cache_dir.as_ref().map(|p| p.display().to_string())),
        ("--target-dir", args.target_dir.as_ref().map(|p| p.display().to_string())),
    ] {
        if let Some(v) = value {
            cmd.args([flag, &v]);
        }
    }
    if !lint_flags.is_empty() {
        let mut flags = std::env::var("RUSTFLAGS").unwrap_or_default();
        for f in lint_flags {
            if !flags.is_empty() {
                flags.push(' ');
            }
            flags.push_str(f);
        }
        cmd.env("RUSTFLAGS", flags);
    }
    if json {
        cmd.stderr(std::process::Stdio::piped());
        cmd.output().context("run mitts check")
    } else {
        let status = cmd.status().context("run mitts check")?;
        Ok(std::process::Output {
            status,
            stdout: Vec::new(),
            stderr: Vec::new(),
        })
    }
}

/// One replacement in a file: bytes `start..end` become `text`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Replacement {
    pub file: PathBuf,
    pub start: usize,
    pub end: usize,
    pub text: String,
}

/// Machine-applicable suggestions for files under `ws`, grouped per diagnostic (a
/// suggestion's parts go in together or not at all), and whether any error was reported.
pub fn suggestions(output: &std::process::Output, ws: &Path) -> (Vec<Vec<Replacement>>, bool) {
    let mut out: Vec<Vec<Replacement>> = Vec::new();
    let mut seen: HashSet<Vec<Replacement>> = HashSet::new();
    let mut errors = false;
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if value["reason"] != "compiler-message" {
            continue;
        }
        let message = &value["message"];
        if message["level"] == "error" {
            errors = true;
        }
        let manifest_dir = value["manifest_path"]
            .as_str()
            .and_then(|m| Path::new(m).parent().map(Path::to_path_buf));
        let mut group = Vec::new();
        collect(message, manifest_dir.as_deref(), &mut group);
        group.retain(|r| r.file.starts_with(ws));
        group.sort();
        group.dedup();
        if !group.is_empty() && seen.insert(group.clone()) {
            out.push(group);
        }
    }
    (out, errors)
}

fn collect(message: &Value, base: Option<&Path>, out: &mut Vec<Replacement>) {
    for span in message["spans"].as_array().into_iter().flatten() {
        if span["suggestion_applicability"] != "MachineApplicable" {
            continue;
        }
        let Some(text) = span["suggested_replacement"].as_str() else {
            continue;
        };
        let Some(file) = span["file_name"].as_str() else {
            continue;
        };
        let path = Path::new(file);
        let path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            base.map(|b| b.join(path)).unwrap_or_else(|| path.to_path_buf())
        };
        let path = path.canonicalize().unwrap_or(path);
        out.push(Replacement {
            file: path,
            start: span["byte_start"].as_u64().unwrap_or(0) as usize,
            end: span["byte_end"].as_u64().unwrap_or(0) as usize,
            text: text.to_string(),
        });
    }
    for child in message["children"].as_array().into_iter().flatten() {
        collect(child, base, out);
    }
}

/// Applies suggestion groups whose parts do not overlap anything already taken;
/// returns how many groups changed each file.
pub fn apply(groups: &[Vec<Replacement>]) -> Result<BTreeMap<PathBuf, usize>> {
    let mut taken: BTreeMap<PathBuf, Vec<(usize, usize)>> = BTreeMap::new();
    let mut chosen: BTreeMap<PathBuf, Vec<&Replacement>> = BTreeMap::new();
    let mut counts: BTreeMap<PathBuf, usize> = BTreeMap::new();
    for group in groups {
        let clashes = group.iter().any(|r| {
            taken.get(&r.file).is_some_and(|ranges| {
                ranges
                    .iter()
                    .any(|&(s, e)| r.start < e && s < r.end || (r.start == r.end && r.start == s && s == e))
            })
        });
        if clashes {
            continue;
        }
        let mut files = HashSet::new();
        for r in group {
            taken.entry(r.file.clone()).or_default().push((r.start, r.end));
            chosen.entry(r.file.clone()).or_default().push(r);
            files.insert(r.file.clone());
        }
        for f in files {
            *counts.entry(f).or_default() += 1;
        }
    }
    for (file, mut edits) in chosen {
        let mut text = std::fs::read(&file).with_context(|| format!("read {}", file.display()))?;
        edits.sort_by(|a, b| b.start.cmp(&a.start));
        for r in edits {
            if r.end > text.len() || r.start > r.end {
                bail!("a suggestion for {} is out of range; the file changed", file.display());
            }
            text.splice(r.start..r.end, r.text.bytes());
        }
        std::fs::write(&file, text)?;
    }
    Ok(counts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggestions_are_applied_back_to_front_without_overlaps() {
        let dir = tempfile::TempDir::new().unwrap();
        let file = dir.path().join("lib.rs");
        std::fs::write(&file, "let mut a = 1; let mut b = 2;").unwrap();
        let file = file.canonicalize().unwrap();
        let r = |start, end, text: &str| Replacement {
            file: file.clone(),
            start,
            end,
            text: text.into(),
        };
        let groups = vec![
            vec![r(4, 8, "")],
            vec![r(19, 23, "")],
            vec![r(4, 9, "x")],
        ];
        let counts = apply(&groups).unwrap();
        assert_eq!(counts[&file], 2);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "let a = 1; let b = 2;");
    }

    #[test]
    fn only_machine_applicable_suggestions_in_the_workspace_count() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("lib.rs"), "x").unwrap();
        let ws = dir.path().canonicalize().unwrap();
        let line = |applicability: &str, file: &str| {
            serde_json::json!({
                "reason": "compiler-message",
                "manifest_path": ws.join("Cargo.toml"),
                "message": {"level": "warning", "spans": [], "children": [{"spans": [{
                    "file_name": file, "byte_start": 0, "byte_end": 1,
                    "suggested_replacement": "y", "suggestion_applicability": applicability
                }]}]}
            })
            .to_string()
        };
        let stdout = [
            line("MachineApplicable", "lib.rs"),
            line("MaybeIncorrect", "lib.rs"),
            line("MachineApplicable", "/elsewhere/dep.rs"),
            line("MachineApplicable", "lib.rs"),
        ]
        .join("\n");
        let output = std::process::Output {
            status: std::process::Command::new("true").status().unwrap(),
            stdout: stdout.into_bytes(),
            stderr: Vec::new(),
        };
        let (groups, errors) = suggestions(&output, &ws);
        assert!(!errors);
        assert_eq!(groups.len(), 1);
    }
}
