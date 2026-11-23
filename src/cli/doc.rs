/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! `mitts doc` and `mitts rustdoc`: a check build gives every crate's metadata, then
//! rustdoc documents each library (and the selected packages' binaries) into
//! `target/doc`, as Cargo does. A crate whose rustdoc command is unchanged is not
//! documented again.

use crate::adapter::rust::dag_unit_externs::resolve_extern_crates;
use crate::adapter::rust::types::PlannedCompilationUnit;
use crate::adapter::{BuildMode, RustAdapter, RustBuildOptions};
use crate::core::hash::Sha256;
use anyhow::{bail, Context, Result};
use clap::Args;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Args, Debug, Clone, Default)]
pub struct DocArgs {
    /// Opens the docs in a browser after the operation
    #[arg(long)]
    pub open: bool,

    /// Don't build documentation for dependencies
    #[arg(long)]
    pub no_deps: bool,

    /// Document private items
    #[arg(long)]
    pub document_private_items: bool,

    /// Package to document
    #[arg(short = 'p', long = "package", value_name = "SPEC", action = clap::ArgAction::Append)]
    pub packages: Vec<String>,

    /// Document all packages in the workspace
    #[arg(long, visible_alias = "all")]
    pub workspace: bool,

    /// Exclude packages from the build
    #[arg(long, value_name = "SPEC", action = clap::ArgAction::Append)]
    pub exclude: Vec<String>,

    /// Document only this package's library
    #[arg(long)]
    pub lib: bool,

    /// Document only the specified binary
    #[arg(long, value_name = "NAME", action = clap::ArgAction::Append)]
    pub bin: Vec<String>,

    /// Document all binaries
    #[arg(long)]
    pub bins: bool,

    /// Space or comma separated list of features to activate
    #[arg(short = 'F', long, value_delimiter = ',', num_args = 1..)]
    pub features: Vec<String>,

    /// Activate all available features
    #[arg(long)]
    pub all_features: bool,

    /// Do not activate the `default` feature
    #[arg(long)]
    pub no_default_features: bool,

    /// Build for the target triple
    #[arg(long)]
    pub target: Option<String>,

    /// Directory for all generated artifacts
    #[arg(long)]
    pub target_dir: Option<PathBuf>,

    /// Build artifacts in release mode, with optimizations
    #[arg(short, long)]
    pub release: bool,

    /// Build artifacts with the specified profile
    #[arg(long, value_name = "PROFILE-NAME")]
    pub profile: Option<String>,

    /// Number of parallel jobs
    #[arg(short = 'j', long, value_name = "N", value_parser = clap::value_parser!(u64).range(1..))]
    pub jobs: Option<u64>,

    /// Do not abort the build as soon as there is an error
    #[arg(long)]
    pub keep_going: bool,

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

    /// Override a Cargo configuration value
    #[arg(long, action = clap::ArgAction::Append)]
    pub config: Vec<String>,

    /// Do not print Mitts log messages
    #[arg(short, long)]
    pub quiet: bool,

    /// Use verbose output
    #[arg(short, long, action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// Error format: human, short, json (passed to the check build)
    #[arg(long, default_value_t = crate::cli::message_format::MessageFormat::Human, value_name = "FMT")]
    pub message_format: crate::cli::message_format::MessageFormat,

    /// Extra arguments for rustdoc (`mitts rustdoc` only)
    #[arg(last = true)]
    pub extra_args: Vec<String>,
}

pub fn handle_doc(args: &DocArgs) -> i32 {
    report(run(args, false))
}

pub fn handle_rustdoc(args: &DocArgs) -> i32 {
    report(run(args, true))
}

fn report(result: Result<i32>) -> i32 {
    match result {
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

fn options(args: &DocArgs) -> RustBuildOptions {
    let mut opt = RustBuildOptions {
        manifest_path: super::manifest_discovery::discover_manifest(&args.manifest_path),
        package: args.packages.first().cloned(),
        packages: args.packages.iter().skip(1).cloned().collect(),
        release: args.release,
        target_dir: args.target_dir.clone(),
        features: args.features.clone(),
        no_default_features: args.no_default_features,
        all_features: args.all_features,
        cache_dir: args.cache_dir.clone(),
        target_triple: args.target.clone(),
        locked: args.locked,
        offline: args.offline,
        frozen: args.frozen,
        native: true,
        verbose: args.verbose,
        mode: BuildMode::Check,
        exclude: args.exclude.clone(),
        lib_only: args.lib,
        bin_targets: args.bin.clone(),
        all_bins: args.bins,
        keep_going: args.keep_going,
        quiet: true,
        // rustdoc needs every crate's metadata path: a fast-lock hit reports none.
        fast_lock: Some(false),
        ..Default::default()
    };
    if args.workspace {
        opt.extra_args.push("--workspace".to_string());
    }
    opt
}

fn run(args: &DocArgs, single: bool) -> Result<i32> {
    crate::adapter::rust::cargo_config_cli::set(&args.config)?;
    let mut opt = options(args);
    super::manifest_discovery::select_workspace_member(&mut opt, args.workspace);
    if let Some(code) = super::profile_flag::apply_or_exit(&mut opt, args.profile.as_deref()) {
        return Ok(code);
    }
    if single && (args.workspace || args.packages.len() > 1) {
        bail!("`mitts rustdoc` documents one package; select it with -p");
    }
    opt.progress = Some(super::build::create_quiet_progress_callback());
    let (plan, result) = RustAdapter::build(&opt)?;
    if !result.success {
        if !result.stderr.is_empty() {
            eprint!("{}", result.stderr);
            if !result.stderr.ends_with('\n') {
                eprintln!();
            }
        }
        return Ok(result.exit_code.max(1));
    }
    let mut explain_opt = opt.clone();
    explain_opt.progress = None;
    let explanation = RustAdapter::explain_plan(&explain_opt)?;
    let completed: HashMap<String, PathBuf> = plan
        .artifacts
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let doc_dir = match &opt.target_triple {
        Some(t) => plan.target_directory.join(t).join("doc"),
        None => plan.target_directory.join("doc"),
    };
    std::fs::create_dir_all(doc_dir.join(".mitts-stamps"))?;
    let rustdoc = crate::adapter::rust::compiler_resolve::resolve_rustdoc();
    let start_dir = opt
        .manifest_path
        .as_ref()
        .and_then(|m| m.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."));
    let flags_target = opt.target_triple.clone().unwrap_or_default();
    let rustdocflags =
        crate::adapter::rust::cargo_config_tools::rustdocflags(&start_dir, &flags_target)?;

    let units = selected_units(&explanation.units, args, single);
    let started = std::time::Instant::now();
    let mut generated: Vec<PathBuf> = Vec::new();
    for unit in &units {
        let local = unit.is_workspace;
        let mut cmd = Command::new(&rustdoc);
        let crate_name = unit.target_name.replace('-', "_");
        let crate_type = if unit.kind == "proc-macro" {
            "proc-macro"
        } else if unit.kind == "bin" {
            "bin"
        } else {
            "lib"
        };
        let mut argv: Vec<String> = vec![
            format!("--edition={}", unit.edition.clone().unwrap_or_else(|| "2015".into())),
            "--crate-type".into(),
            crate_type.into(),
            "--crate-name".into(),
            crate_name.clone(),
            unit.src_path.to_string_lossy().into_owned(),
            "-o".into(),
            doc_dir.to_string_lossy().into_owned(),
        ];
        let mut features = unit.features.clone();
        features.sort();
        features.dedup();
        for f in features {
            argv.push("--cfg".into());
            argv.push(format!("feature=\"{f}\""));
        }
        for cfg in plan.unit_cfgs.get(&unit.unit_id()).into_iter().flatten() {
            argv.push("--cfg".into());
            argv.push(cfg.clone());
        }
        let externs = resolve_extern_crates(unit, &completed);
        let dirs: HashSet<PathBuf> = externs
            .iter()
            .filter_map(|(_, p)| p.parent().map(Path::to_path_buf))
            .collect();
        let mut dirs: Vec<PathBuf> = dirs.into_iter().collect();
        dirs.sort();
        for dir in dirs {
            argv.push("-L".into());
            argv.push(format!("dependency={}", dir.display()));
        }
        for (name, path) in &externs {
            if *name != crate_name || unit.kind == "bin" {
                argv.push("--extern".into());
                argv.push(format!("{name}={}", path.display()));
            }
        }
        argv.push("--crate-version".into());
        argv.push(unit.version.clone());
        if !local {
            argv.push("--cap-lints".into());
            argv.push("allow".into());
        }
        if (args.document_private_items || unit.kind == "bin") && local {
            argv.push("--document-private-items".into());
        }
        argv.extend(rustdocflags.iter().cloned());
        if single && unit.is_primary {
            argv.extend(args.extra_args.iter().cloned());
        }
        let mut env = crate::adapter::rust::unit_env::compute_unit_rustc_env(unit, None, None);
        if let Some(extra) = plan.unit_env.get(&unit.unit_id()) {
            env.extend(extra.clone());
        }
        let mut sorted_env: Vec<(&String, &String)> = env.iter().collect();
        sorted_env.sort();
        // The stamp: rustdoc, its arguments, environment and the bytes of the crate's own
        // metadata and of its dependencies' (paths of staging directories vary per run).
        let mut inputs = String::new();
        inputs.push_str(&rustdoc.to_string_lossy());
        let mut skip_value = false;
        for a in &argv {
            if skip_value {
                skip_value = false;
                continue;
            }
            if a == "-L" || a == "--extern" {
                skip_value = true;
                continue;
            }
            inputs.push('\n');
            inputs.push_str(a);
        }
        for (name, path) in &externs {
            inputs.push_str(&format!("\nextern {name} {}", file_digest(path)));
        }
        // A registry or git package is immutable at its version; a local one is read
        // from its directory.
        if unit.is_workspace || unit.package_id.contains("path+") || !unit.package_id.contains(" (") {
            if let Some(dir) = unit.manifest_path.parent() {
                inputs.push_str(&format!("\nsources {}", tree_digest(dir)));
            }
        } else {
            inputs.push_str(&format!("\npackage {}", unit.package_id));
        }
        for (k, v) in &sorted_env {
            inputs.push_str(&format!("\nenv {k}={v}"));
        }
        let mut hasher = Sha256::new();
        hasher.update(inputs.as_bytes());
        let stamp = doc_dir
            .join(".mitts-stamps")
            .join(format!(
                "{crate_name}-{}",
                crate::core::hash::bytes_to_hex(&hasher.finalize())
            ));
        let index = doc_dir.join(&crate_name).join("index.html");
        if unit.is_primary {
            generated.push(index.clone());
        }
        if stamp.is_file() && index.is_file() {
            if args.verbose > 0 && !args.quiet {
                status("Fresh", &format!("{} v{}", unit.package_name, unit.version));
            }
            continue;
        }
        if !args.quiet {
            status("Documenting", &format!("{} v{}", unit.package_name, unit.version));
        }
        cmd.args(&argv).envs(&env);
        if let Some(dir) = unit.manifest_path.parent() {
            cmd.current_dir(dir);
        }
        if args.verbose > 0 {
            eprintln!("     Running `{} {}`", rustdoc.display(), argv.join(" "));
        }
        let status_code = cmd
            .status()
            .with_context(|| format!("failed to run `{}`", rustdoc.display()))?;
        if !status_code.success() {
            bail!(
                "could not document `{}`",
                unit.package_name
            );
        }
        let _ = std::fs::write(&stamp, inputs.as_bytes());
    }
    if !args.quiet {
        let profile = crate::adapter::rust::profile::profile_name(opt.release);
        status(
            "Finished",
            &format!("`{profile}` profile target(s) in {:.2}s", started.elapsed().as_secs_f64()),
        );
        match generated.as_slice() {
            [] => {}
            [one] => status("Generated", &one.display().to_string()),
            [first, rest @ ..] => status(
                "Generated",
                &format!(
                    "{} and {} other file{}",
                    first.display(),
                    rest.len(),
                    if rest.len() == 1 { "" } else { "s" }
                ),
            ),
        }
    }
    if args.open {
        if let Some(index) = generated.first() {
            open(index, args.quiet);
        }
    }
    Ok(0)
}

/// The crates to document: every library (or only the selected packages' with
/// `--no-deps` / `mitts rustdoc`), plus the selected packages' binaries unless a library
/// of the same crate name is documented, once each.
fn selected_units(
    units: &[PlannedCompilationUnit],
    args: &DocArgs,
    single: bool,
) -> Vec<PlannedCompilationUnit> {
    let mut out: Vec<PlannedCompilationUnit> = Vec::new();
    let mut seen: HashSet<(String, String, String)> = HashSet::new();
    let lib_names: HashSet<(String, String)> = units
        .iter()
        .filter(|u| u.is_primary && (u.kind == "proc-macro" || is_lib(&u.kind)))
        .map(|u| (u.package_name.clone(), u.target_name.replace('-', "_")))
        .collect();
    for unit in units {
        let lib = unit.kind == "proc-macro" || is_lib(&unit.kind);
        let wanted = if lib {
            unit.is_primary || !(args.no_deps || single)
        } else if unit.kind == "bin" {
            unit.is_primary
                && !args.lib
                && !lib_names.contains(&(unit.package_name.clone(), unit.target_name.replace('-', "_")))
        } else {
            false
        };
        if !wanted {
            continue;
        }
        let key = (unit.package_name.clone(), unit.version.clone(), unit.target_name.clone());
        if seen.insert(key) {
            out.push(unit.clone());
        }
    }
    // Dependencies first, then the selected packages, as Cargo reports them.
    out.sort_by_key(|u| u.is_primary);
    out
}

fn file_digest(path: &Path) -> String {
    crate::core::hash::Sha256::digest_file(path).unwrap_or_else(|_| path.display().to_string())
}

/// Digest of a local package's files, leaving out `target/` and hidden entries.
fn tree_digest(dir: &Path) -> String {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<(String, String)>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') || (dir == base && name == "target") {
                continue;
            }
            let path = entry.path();
            match entry.file_type() {
                Ok(t) if t.is_dir() => walk(base, &path, out),
                Ok(t) if t.is_file() => {
                    let rel = path.strip_prefix(base).unwrap_or(&path).display().to_string();
                    out.push((rel, file_digest(&path)));
                }
                _ => {}
            }
        }
    }
    let mut files = Vec::new();
    walk(dir, dir, &mut files);
    files.sort();
    let mut hasher = Sha256::new();
    for (rel, digest) in files {
        hasher.update(rel.as_bytes());
        hasher.update(digest.as_bytes());
    }
    crate::core::hash::bytes_to_hex(&hasher.finalize())
}

fn is_lib(kind: &str) -> bool {
    crate::adapter::rust::planner_units::is_library_kind(kind)
}

/// Opens a page like Cargo: `doc.browser`, `BROWSER`, else the system opener.
fn open(path: &Path, quiet: bool) {
    if !quiet {
        status("Opening", &path.display().to_string());
    }
    let browser = std::env::var("BROWSER").ok().filter(|b| !b.is_empty());
    let result = match browser {
        Some(b) => Command::new(b).arg(path).status(),
        None if cfg!(target_os = "macos") => Command::new("open").arg(path).status(),
        None if cfg!(windows) => Command::new("cmd").args(["/C", "start", ""]).arg(path).status(),
        None => Command::new("xdg-open").arg(path).status(),
    };
    if !result.is_ok_and(|s| s.success()) {
        eprintln!("warning: could not open {} in a browser", path.display());
    }
}
