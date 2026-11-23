/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! `mitts install` and `mitts uninstall`: a package from a registry, a path or git built
//! in release with Mitts (and its shared cache), its binaries copied to `<root>/bin`, and
//! the install recorded in Cargo's `.crates.toml` / `.crates2.json`.

use super::registry_api::download_crate;
use anyhow::{bail, Context, Result};
use clap::Args;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Args, Debug, Clone, Default)]
pub struct InstallArgs {
    /// Packages to install: NAME or NAME@VERSION
    #[arg(value_name = "CRATE")]
    pub crates: Vec<String>,

    /// Specify a version to install
    #[arg(long = "version", visible_alias = "vers", value_name = "VERSION")]
    pub version: Option<String>,

    /// Git URL to install the specified crate from
    #[arg(long)]
    pub git: Option<String>,

    /// Branch to use when installing from git
    #[arg(long, requires = "git")]
    pub branch: Option<String>,

    /// Tag to use when installing from git
    #[arg(long, requires = "git")]
    pub tag: Option<String>,

    /// Specific commit to use when installing from git
    #[arg(long, requires = "git")]
    pub rev: Option<String>,

    /// Filesystem path to local crate to install from
    #[arg(long)]
    pub path: Option<PathBuf>,

    /// List all installed packages and their versions
    #[arg(long)]
    pub list: bool,

    /// Force overwriting existing crates or binaries
    #[arg(short, long)]
    pub force: bool,

    /// Do not save tracking information
    #[arg(long)]
    pub no_track: bool,

    /// Install only the specified binary
    #[arg(long, value_name = "NAME", action = clap::ArgAction::Append)]
    pub bin: Vec<String>,

    /// Install all binaries
    #[arg(long)]
    pub bins: bool,

    /// Install only the specified example
    #[arg(long, value_name = "NAME", action = clap::ArgAction::Append)]
    pub example: Vec<String>,

    /// Install all examples
    #[arg(long)]
    pub examples: bool,

    /// Directory to install packages into
    #[arg(long)]
    pub root: Option<PathBuf>,

    /// Build in debug mode (with the 'dev' profile) instead of release mode
    #[arg(long)]
    pub debug: bool,

    /// Install artifacts with the specified profile
    #[arg(long, value_name = "PROFILE-NAME")]
    pub profile: Option<String>,

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

    /// Number of parallel jobs
    #[arg(short = 'j', long, value_name = "N")]
    pub jobs: Option<u64>,

    /// Directory for the shared artifact and action cache
    #[arg(long)]
    pub cache_dir: Option<PathBuf>,

    /// Registry to use
    #[arg(long)]
    pub registry: Option<String>,

    /// Registry index to install from
    #[arg(long)]
    pub index: Option<String>,

    /// Use the package's own Cargo.lock
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

#[derive(Args, Debug, Clone, Default)]
pub struct UninstallArgs {
    /// Packages to uninstall
    #[arg(value_name = "SPEC")]
    pub specs: Vec<String>,

    /// Package to uninstall
    #[arg(short = 'p', long = "package", value_name = "SPEC", action = clap::ArgAction::Append)]
    pub packages: Vec<String>,

    /// Only uninstall the binary NAME
    #[arg(long, value_name = "NAME", action = clap::ArgAction::Append)]
    pub bin: Vec<String>,

    /// Directory to uninstall packages from
    #[arg(long)]
    pub root: Option<PathBuf>,

    /// Do not print Mitts log messages
    #[arg(short, long)]
    pub quiet: bool,

    /// Use verbose output
    #[arg(short, long, action = clap::ArgAction::Count)]
    pub verbose: u8,
}

fn status(word: &str, message: &str) {
    eprintln!("{word:>12} {message}");
}

fn report(result: Result<()>) -> i32 {
    match result {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("error: {error:#}");
            101
        }
    }
}

/// `--root`, `CARGO_INSTALL_ROOT`, `install.root` config, else the Cargo home.
pub fn install_root(flag: &Option<PathBuf>) -> Result<PathBuf> {
    if let Some(root) = flag {
        return Ok(std::path::absolute(root)?);
    }
    if let Some(root) = std::env::var_os("CARGO_INSTALL_ROOT").filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(root));
    }
    let cwd = std::env::current_dir()?;
    if let Some(value) = crate::adapter::rust::cargo_config_value::get(&cwd, "install.root")? {
        return value.as_path();
    }
    crate::core::source_store::SourceStore::active_cargo_home()
        .context("cannot find the Cargo home directory; pass --root")
}

/// Cargo's install records: `.crates.toml` (package id -> binaries).
fn read_tracking(root: &Path) -> BTreeMap<String, Vec<String>> {
    let text = std::fs::read_to_string(root.join(".crates.toml")).unwrap_or_default();
    let value: toml::Value = toml::from_str(&text).unwrap_or(toml::Value::Table(Default::default()));
    value
        .get("v1")
        .and_then(|v| v.as_table())
        .map(|t| {
            t.iter()
                .map(|(k, v)| {
                    let bins = v
                        .as_array()
                        .map(|a| a.iter().filter_map(|b| b.as_str().map(str::to_string)).collect())
                        .unwrap_or_default();
                    (k.clone(), bins)
                })
                .collect()
        })
        .unwrap_or_default()
}

fn write_tracking(root: &Path, entries: &BTreeMap<String, Vec<String>>, extra: Option<(&str, Value)>) -> Result<()> {
    let mut v1 = toml::value::Table::new();
    for (id, bins) in entries {
        v1.insert(
            id.clone(),
            toml::Value::Array(bins.iter().map(|b| toml::Value::String(b.clone())).collect()),
        );
    }
    let mut doc = toml::value::Table::new();
    doc.insert("v1".into(), toml::Value::Table(v1));
    std::fs::write(root.join(".crates.toml"), toml::to_string(&doc)?)?;
    let path2 = root.join(".crates2.json");
    let mut v2: Value = std::fs::read_to_string(&path2)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| json!({"installs": {}}));
    if let Some(installs) = v2["installs"].as_object_mut() {
        installs.retain(|k, _| entries.contains_key(k));
        if let Some((id, info)) = extra {
            installs.insert(id.to_string(), info);
        }
    }
    std::fs::write(path2, serde_json::to_string(&v2)?)?;
    Ok(())
}

pub fn handle_install(args: &InstallArgs) -> i32 {
    report(run_install(args))
}

fn run_install(args: &InstallArgs) -> Result<()> {
    let root = install_root(&args.root)?;
    if args.list {
        for (id, bins) in read_tracking(&root) {
            let mut words = id.split(' ');
            let name = words.next().unwrap_or_default();
            let version = words.next().unwrap_or_default();
            println!("{name} v{version}:");
            for bin in bins {
                println!("    {bin}");
            }
        }
        return Ok(());
    }
    let mut specs: Vec<String> = args.crates.clone();
    if specs.is_empty() && args.path.is_none() && args.git.is_none() {
        bail!("Using `mitts install` to install the binaries from the package in the current working directory is not supported, use `mitts install --path .` instead.");
    }
    if specs.is_empty() {
        specs.push(String::new());
    }
    for spec in specs {
        install_one(args, &root, &spec)?;
    }
    let bin_dir = root.join("bin");
    let on_path = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d == bin_dir))
        .unwrap_or(false);
    if !on_path && !args.quiet {
        eprintln!(
            "warning: be sure to add `{}` to your PATH to be able to run the installed binaries",
            bin_dir.display()
        );
    }
    Ok(())
}

struct Source {
    dir: PathBuf,
    name: String,
    version: String,
    /// Cargo's source id for `.crates.toml`.
    source_id: String,
    temp: Option<PathBuf>,
}

impl Drop for Source {
    fn drop(&mut self) {
        if let Some(t) = &self.temp {
            let _ = std::fs::remove_dir_all(t);
        }
    }
}

fn scratch(label: &str) -> Result<PathBuf> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("mitts-install-{label}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn package_of(dir: &Path, wanted: Option<&str>) -> Result<(PathBuf, String, String)> {
    let manifest = dir.join("Cargo.toml");
    let toml = crate::adapter::rust::manifest::CargoToml::from_file(&manifest)
        .with_context(|| format!("could not read `{}`", manifest.display()))?;
    if let Some(p) = &toml.package {
        if wanted.is_none_or(|w| w.is_empty() || w == p.name) {
            return Ok((
                dir.to_path_buf(),
                p.name.clone(),
                p.version.clone().unwrap_or_else(|| "0.0.0".into()),
            ));
        }
    }
    if toml.workspace.is_some() {
        let members = toml.discover_workspace_members(dir)?;
        let mut with_bins = Vec::new();
        for (name, member_dir) in members {
            if wanted.is_some_and(|w| !w.is_empty() && w != name) {
                continue;
            }
            let m = crate::adapter::rust::manifest::CargoToml::from_file(&member_dir.join("Cargo.toml"))?;
            if m.discover_targets(&member_dir).iter().any(|t| t.kind == "bin") {
                with_bins.push((member_dir, name, m.package.and_then(|p| p.version).unwrap_or_else(|| "0.0.0".into())));
            }
        }
        match with_bins.len() {
            1 => return Ok(with_bins.remove(0)),
            0 => {}
            _ => bail!("multiple packages with binaries found; name the one to install"),
        }
    }
    bail!("could not find `{}` in `{}`", wanted.unwrap_or("a package"), dir.display())
}

fn locate(args: &InstallArgs, spec: &str) -> Result<Source> {
    let (wanted_name, spec_version) = if spec.is_empty() {
        ("", None)
    } else {
        super::project_info::parse_spec(spec)
    };
    if let Some(path) = &args.path {
        let abs = std::path::absolute(path)?;
        let (dir, name, version) = package_of(&abs, Some(wanted_name))?;
        let canonical = dir.canonicalize().unwrap_or(dir.clone());
        return Ok(Source {
            source_id: format!("path+file://{}", canonical.display()),
            dir,
            name,
            version,
            temp: None,
        });
    }
    if let Some(url) = &args.git {
        let temp = scratch("git")?;
        let checkout = temp.join("repo");
        let mut clone = Command::new(std::env::var("MITTS_GIT").unwrap_or_else(|_| "git".into()));
        clone.args(["clone", "--quiet"]);
        if let Some(b) = args.branch.as_ref().or(args.tag.as_ref()) {
            clone.args(["--branch", b]);
        }
        clone.arg(url).arg(&checkout);
        if !clone.status().is_ok_and(|s| s.success()) {
            bail!("failed to clone `{url}`");
        }
        if let Some(rev) = &args.rev {
            let ok = Command::new("git")
                .args(["checkout", "--quiet", rev])
                .current_dir(&checkout)
                .status()
                .is_ok_and(|s| s.success());
            if !ok {
                bail!("failed to check out `{rev}` in `{url}`");
            }
        }
        let head = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&checkout)
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        let (dir, name, version) = package_of(&checkout, Some(wanted_name))?;
        let query = match (&args.branch, &args.tag, &args.rev) {
            (Some(b), _, _) => format!("?branch={b}"),
            (_, Some(t), _) => format!("?tag={t}"),
            (_, _, Some(r)) => format!("?rev={r}"),
            _ => String::new(),
        };
        return Ok(Source {
            source_id: format!("git+{url}{query}#{head}"),
            dir,
            name,
            version,
            temp: Some(temp),
        });
    }
    // A registry package.
    if wanted_name.is_empty() {
        bail!("a package name is required");
    }
    let cwd = std::env::current_dir()?;
    let config = crate::adapter::rust::registry_config::RegistryConfig::load(&cwd)?;
    let resolver = crate::adapter::rust::resolver::NativeResolver::for_cache_dir(args.cache_dir.as_deref())?;
    if !args.quiet && !args.offline {
        status("Updating", &format!("{} index", args.registry.as_deref().unwrap_or("crates.io")));
    }
    let versions = resolver
        .registry_versions(&cwd, args.registry.as_deref(), wanted_name, args.offline || args.frozen)
        .with_context(|| format!("could not find `{wanted_name}` in registry"))?;
    let req = spec_version
        .map(|v| {
            if v.chars().next().is_some_and(|c| c.is_ascii_digit()) && v.split('.').count() == 3 {
                format!("={v}")
            } else {
                v.to_string()
            }
        })
        .or_else(|| args.version.clone());
    let req = req
        .as_deref()
        .map(|r| semver::VersionReq::parse(r).with_context(|| format!("invalid version requirement `{r}`")))
        .transpose()?;
    let entry = versions
        .iter()
        .filter(|e| !e.yanked)
        .filter_map(|e| semver::Version::parse(&e.vers).ok().map(|v| (v, e)))
        .filter(|(v, _)| req.as_ref().map_or(v.pre.is_empty(), |r| r.matches(v)))
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, e)| e.clone())
        .with_context(|| match &req {
            Some(r) => format!("could not find `{wanted_name}` in registry with version `{r}`"),
            None => format!("could not find `{wanted_name}` in registry"),
        })?;
    let index = match &args.index {
        Some(i) => i.trim_start_matches("sparse+").to_string(),
        None => config.index_url_for(args.registry.as_deref())?,
    };
    let bytes = download_crate(&index, &entry.name, &entry.vers, Some(&entry.cksum))?;
    if !args.quiet {
        status("Downloaded", &format!("{} v{}", entry.name, entry.vers));
    }
    let temp = scratch(&entry.name)?;
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(&bytes[..]));
    archive.unpack(&temp)?;
    let dir = temp.join(format!("{}-{}", entry.name, entry.vers));
    let source_id = config.lock_source_for_registry(args.registry.as_deref())?;
    Ok(Source {
        dir,
        name: entry.name.clone(),
        version: entry.vers.clone(),
        source_id,
        temp: Some(temp),
    })
}

fn install_one(args: &InstallArgs, root: &Path, spec: &str) -> Result<()> {
    let source = locate(args, spec)?;
    let id = format!("{} {} ({})", source.name, source.version, source.source_id);
    let tracking = read_tracking(root);
    if tracking.contains_key(&id) && !args.force {
        if !args.quiet {
            status(
                "Ignored",
                &format!(
                    "package `{} v{}` is already installed, use --force to override",
                    source.name, source.version
                ),
            );
        }
        return Ok(());
    }
    if !args.quiet {
        status("Installing", &format!("{} v{}", source.name, source.version));
    }
    if !args.locked && !args.frozen && source.temp.is_some() {
        let _ = std::fs::remove_file(source.dir.join("Cargo.lock"));
    }
    let manifest = source.dir.join("Cargo.toml");
    let toml = crate::adapter::rust::manifest::CargoToml::from_file(&manifest)?;
    let targets = toml.discover_targets(&source.dir);
    let examples = !args.example.is_empty() || args.examples;
    let mut chosen: Vec<String> = targets
        .iter()
        .filter(|t| {
            if examples {
                t.kind == "example" && (args.examples || args.example.contains(&t.name))
            } else {
                t.kind == "bin" && (args.bin.is_empty() || args.bin.contains(&t.name))
            }
        })
        .map(|t| t.name.clone())
        .collect();
    chosen.sort();
    if chosen.is_empty() {
        bail!(
            "there is nothing to install in `{} v{}`, because it has no binaries\n\
             `mitts install` is only for installing programs, and can't be used with libraries.\n\
             To use a library crate, add it as a dependency to a Cargo project with `mitts add`.",
            source.name,
            source.version
        );
    }
    for bin in args.bin.iter().chain(args.example.iter()) {
        if !chosen.contains(bin) {
            bail!("no {} target named `{bin}` in `{}`", if examples { "example" } else { "bin" }, source.name);
        }
    }
    // Refuse to overwrite another package's binaries unless forced, as Cargo does.
    if !args.force {
        for (other, bins) in &tracking {
            if other.starts_with(&format!("{} ", source.name)) {
                continue;
            }
            if let Some(bin) = bins.iter().find(|b| chosen.contains(b)) {
                bail!("binary `{bin}` already exists in destination as part of `{other}`\nAdd --force to overwrite");
            }
        }
    }

    let build_root = match &args.target_dir {
        Some(d) => std::path::absolute(d)?,
        None => scratch("target")?,
    };
    let exe = std::env::current_exe()?;
    let mut cmd = Command::new(exe);
    cmd.arg("build").arg("--manifest-path").arg(&manifest).arg("--target-dir").arg(&build_root);
    let profile = match (&args.profile, args.debug) {
        (Some(p), _) => p.clone(),
        (None, true) => "dev".to_string(),
        (None, false) => "release".to_string(),
    };
    cmd.args(["--profile", &profile]);
    for bin in &chosen {
        cmd.args([if examples { "--example" } else { "--bin" }, bin]);
    }
    if !args.features.is_empty() {
        cmd.args(["--features", &args.features.join(",")]);
    }
    for (flag, on) in [
        ("--all-features", args.all_features),
        ("--no-default-features", args.no_default_features),
        ("--locked", args.locked),
        ("--offline", args.offline),
        ("--frozen", args.frozen),
        ("--quiet", args.quiet),
    ] {
        if on {
            cmd.arg(flag);
        }
    }
    if let Some(t) = &args.target {
        cmd.args(["--target", t]);
    }
    if let Some(j) = args.jobs {
        cmd.args(["--jobs", &j.to_string()]);
    }
    if let Some(c) = &args.cache_dir {
        cmd.arg("--cache-dir").arg(c);
    }
    let ok = cmd.status().context("run mitts build")?.success();
    let cleanup = |dir: &Path| {
        if args.target_dir.is_none() {
            let _ = std::fs::remove_dir_all(dir);
        }
    };
    if !ok {
        cleanup(&build_root);
        bail!("failed to compile `{} v{}`, intermediate artifacts can be found at `{}`", source.name, source.version, build_root.display());
    }
    let profile_dir = match profile.as_str() {
        "dev" | "test" => "debug".to_string(),
        "bench" => "release".to_string(),
        other => other.to_string(),
    };
    let mut out_dir = build_root.clone();
    if let Some(t) = &args.target {
        out_dir = out_dir.join(t);
    }
    out_dir = out_dir.join(profile_dir);
    if examples {
        out_dir = out_dir.join("examples");
    }
    let bin_dir = root.join("bin");
    std::fs::create_dir_all(&bin_dir)?;
    let suffix = if args.target.as_deref().map_or(cfg!(windows), |t| t.contains("windows")) {
        ".exe"
    } else {
        ""
    };
    let mut installed = Vec::new();
    for bin in &chosen {
        let file = format!("{bin}{suffix}");
        let from = out_dir.join(&file);
        if !from.is_file() {
            cleanup(&build_root);
            bail!("the build did not produce `{}`", from.display());
        }
        let to = bin_dir.join(&file);
        let replacing = to.exists();
        let tmp = bin_dir.join(format!(".{file}.mitts-tmp"));
        std::fs::copy(&from, &tmp)?;
        std::fs::rename(&tmp, &to)?;
        if !args.quiet {
            status(if replacing { "Replacing" } else { "Installing" }, &to.display().to_string());
        }
        installed.push(file);
    }
    cleanup(&build_root);
    if !args.no_track {
        let mut tracking = tracking;
        tracking.retain(|k, _| !k.starts_with(&format!("{} ", source.name)));
        tracking.insert(id.clone(), installed.clone());
        let info = json!({
            "version_req": args.version,
            "bins": installed,
            "features": args.features,
            "all_features": args.all_features,
            "no_default_features": args.no_default_features,
            "profile": profile,
            "target": args.target,
            "rustc": crate::adapter::RustAdapter::detect_toolchain().map(|t| t.version).unwrap_or_default(),
        });
        write_tracking(root, &tracking, Some((&id, info)))?;
    }
    if !args.quiet {
        let noun = if installed.len() == 1 { "executable" } else { "executables" };
        let names: Vec<String> = installed.iter().map(|b| format!("`{b}`")).collect();
        status(
            "Installed",
            &format!("package `{} v{}` ({noun} {})", source.name, source.version, names.join(", ")),
        );
    }
    Ok(())
}

pub fn handle_uninstall(args: &UninstallArgs) -> i32 {
    report((|| {
        let root = install_root(&args.root)?;
        let mut tracking = read_tracking(&root);
        let mut specs: Vec<String> = args.specs.clone();
        specs.extend(args.packages.iter().cloned());
        if specs.is_empty() {
            let manifest = super::project_info::starting_manifest(&None)?;
            let name = crate::adapter::rust::manifest::CargoToml::from_file(&manifest)?
                .package
                .map(|p| p.name)
                .context("a package to uninstall is required")?;
            specs.push(name);
        }
        for spec in specs {
            let (name, version) = super::project_info::parse_spec(&spec);
            let ids: Vec<String> = tracking
                .keys()
                .filter(|id| {
                    let mut w = id.split(' ');
                    w.next() == Some(name) && version.is_none_or(|v| w.next().is_some_and(|x| x.starts_with(v)))
                })
                .cloned()
                .collect();
            let id = match ids.as_slice() {
                [one] => one.clone(),
                [] => bail!("package ID specification `{spec}` did not match any packages"),
                _ => bail!("multiple packages match `{spec}`; give the version too"),
            };
            let bins = tracking.get(&id).cloned().unwrap_or_default();
            let (remove, keep): (Vec<String>, Vec<String>) = bins.into_iter().partition(|b| {
                args.bin.is_empty() || args.bin.iter().any(|x| x == b || format!("{x}.exe") == *b)
            });
            if remove.is_empty() {
                bail!("binary `{}` not installed as part of `{id}`", args.bin.join(", "));
            }
            for bin in &remove {
                let path = root.join("bin").join(bin);
                if !args.quiet {
                    status("Removing", &path.display().to_string());
                }
                if path.exists() {
                    std::fs::remove_file(&path)?;
                }
            }
            if keep.is_empty() {
                tracking.remove(&id);
            } else {
                tracking.insert(id, keep);
            }
        }
        write_tracking(&root, &tracking, None)
    })())
}
