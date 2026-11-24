/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! `mitts search`, `info`, `login`, `logout`, `owner` and `yank`, speaking a registry's
//! web API as Cargo does.

use super::registry_api::{encode, RegistryApi};
use anyhow::{bail, Context, Result};
use clap::Args;
use serde_json::{json, Value};
use std::io::{BufRead, Read};
use std::path::PathBuf;

#[derive(Args, Debug, Clone, Default)]
pub struct RegistryChoice {
    /// Registry to use
    #[arg(long)]
    pub registry: Option<String>,

    /// Registry index URL to use
    #[arg(long)]
    pub index: Option<String>,

    /// Do not print Mitts log messages
    #[arg(short, long)]
    pub quiet: bool,

    /// Use verbose output
    #[arg(short, long, action = clap::ArgAction::Count)]
    pub verbose: u8,
}

#[derive(Args, Debug, Clone, Default)]
pub struct SearchArgs {
    /// Words to search for
    #[arg(value_name = "QUERY")]
    pub query: Vec<String>,

    /// Limit the number of results (default: 10, max: 100)
    #[arg(long, default_value_t = 10)]
    pub limit: u32,

    #[command(flatten)]
    pub registry: RegistryChoice,
}

#[derive(Args, Debug, Clone, Default)]
pub struct InfoArgs {
    /// Package to inspect: NAME or NAME@VERSION
    #[arg(value_name = "SPEC")]
    pub spec: String,

    /// Directory for the shared artifact and action cache
    #[arg(long)]
    pub cache_dir: Option<PathBuf>,

    /// Run without accessing the network
    #[arg(long)]
    pub offline: bool,

    /// Require Cargo.lock is up to date
    #[arg(long)]
    pub locked: bool,

    /// Equivalent to specifying both --locked and --offline
    #[arg(long)]
    pub frozen: bool,

    #[command(flatten)]
    pub registry: RegistryChoice,
}

#[derive(Args, Debug, Clone, Default)]
pub struct LoginArgs {
    /// The token (read from stdin when left out)
    #[arg(value_name = "TOKEN")]
    pub token: Option<String>,

    /// Arguments for the credential provider (accepted, not used by `cargo:token`)
    #[arg(last = true)]
    pub args: Vec<String>,

    #[command(flatten)]
    pub registry: RegistryChoice,
}

#[derive(Args, Debug, Clone, Default)]
pub struct LogoutArgs {
    #[command(flatten)]
    pub registry: RegistryChoice,
}

#[derive(Args, Debug, Clone, Default)]
pub struct OwnerArgs {
    /// Name of the crate (the current package when left out)
    #[arg(value_name = "CRATE")]
    pub krate: Option<String>,

    /// Name of a user or team to invite as an owner
    #[arg(short = 'a', long = "add", value_name = "LOGIN", action = clap::ArgAction::Append)]
    pub add: Vec<String>,

    /// Name of a user or team to remove as an owner
    #[arg(short = 'r', long = "remove", value_name = "LOGIN", action = clap::ArgAction::Append)]
    pub remove: Vec<String>,

    /// List owners of a crate
    #[arg(short = 'l', long)]
    pub list: bool,

    /// API token to use when authenticating
    #[arg(long)]
    pub token: Option<String>,

    #[command(flatten)]
    pub registry: RegistryChoice,
}

#[derive(Args, Debug, Clone, Default)]
pub struct YankArgs {
    /// The crate to yank: NAME or NAME@VERSION
    #[arg(value_name = "CRATE")]
    pub krate: Option<String>,

    /// The version to yank or un-yank
    #[arg(long = "version", visible_alias = "vers", value_name = "VERSION")]
    pub version: Option<String>,

    /// Undo a yank, putting a version back into the index
    #[arg(long)]
    pub undo: bool,

    /// API token to use when authenticating
    #[arg(long)]
    pub token: Option<String>,

    #[command(flatten)]
    pub registry: RegistryChoice,
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

fn project_dir() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

fn open(choice: &RegistryChoice) -> Result<RegistryApi> {
    RegistryApi::open(
        &project_dir(),
        choice.registry.as_deref(),
        choice.index.as_deref(),
    )
}

pub fn handle_search(args: &SearchArgs) -> i32 {
    report((|| {
        let api = open(&args.registry)?;
        let limit = args.limit.clamp(1, 100);
        let query = args.query.join(" ");
        let doc = api.get(
            &format!("/crates?q={}&per_page={limit}", encode(&query)),
            None,
        )?;
        let crates = doc["crates"].as_array().cloned().unwrap_or_default();
        let lines: Vec<(String, String)> = crates
            .iter()
            .map(|c| {
                let version = c["max_stable_version"]
                    .as_str()
                    .or_else(|| c["max_version"].as_str())
                    .unwrap_or("");
                let line = format!("{} = \"{version}\"", c["name"].as_str().unwrap_or(""));
                let desc = c["description"]
                    .as_str()
                    .map(|d| d.split_whitespace().collect::<Vec<_>>().join(" "))
                    .map(|d| truncate(&d, 128))
                    .unwrap_or_default();
                (line, desc)
            })
            .collect();
        let width = lines.iter().map(|(l, _)| l.len()).max().unwrap_or(0) + 4;
        for (line, desc) in &lines {
            if desc.is_empty() {
                println!("{line}");
            } else {
                println!("{line:<width$}# {desc}");
            }
        }
        let total = doc["meta"]["total"].as_u64().unwrap_or(lines.len() as u64);
        let more = total.saturating_sub(lines.len() as u64);
        if more > 0 {
            println!("... and {more} crates more (use --limit N to see more)");
        }
        if !args.registry.quiet && api.name.as_deref() == Some("crates-io") && !lines.is_empty() {
            eprintln!("note: to learn more about a package, run `mitts info <name>`");
        }
        Ok(())
    })())
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let cut: String = text.chars().take(max).collect();
    format!("{cut}...")
}

pub fn handle_info(args: &InfoArgs) -> i32 {
    report(run_info(args))
}

fn run_info(args: &InfoArgs) -> Result<()> {
    let (name, wanted) = super::project_info::parse_spec(&args.spec);
    let dir = project_dir();
    let resolver =
        crate::adapter::rust::resolver::NativeResolver::for_cache_dir(args.cache_dir.as_deref())?;
    let versions = resolver
        .registry_versions(
            &dir,
            args.registry.registry.as_deref(),
            name,
            args.offline || args.frozen,
        )
        .with_context(|| format!("could not find `{name}` in registry"))?;
    if versions.is_empty() {
        bail!("could not find `{name}` in registry");
    }
    // A version the current workspace locks wins, as in Cargo.
    let locked = super::project_info::starting_manifest(&None)
        .ok()
        .and_then(|m| super::project_info::workspace_root(&m).ok())
        .and_then(|root| {
            crate::adapter::rust::lockfile::CargoLock::from_file(&root.with_file_name("Cargo.lock"))
                .ok()
        })
        .and_then(|lock| {
            lock.packages
                .into_iter()
                .find(|p| p.name == name && p.source.is_some())
        })
        .map(|p| p.version);
    let chosen = versions
        .iter()
        .filter(|e| match wanted {
            Some(v) => e.vers == v || e.vers.starts_with(&format!("{v}.")),
            None => locked.as_deref().is_none_or(|l| e.vers == l),
        })
        .filter(|e| !e.yanked || wanted.is_some())
        .filter_map(|e| semver::Version::parse(&e.vers).ok().map(|v| (v, e)))
        .max_by(|a, b| (a.0.pre.is_empty(), &a.0).cmp(&(b.0.pre.is_empty(), &b.0)))
        .map(|(_, e)| e.clone())
        .with_context(|| format!("could not find `{}` in registry", args.spec))?;

    let manifest = download_manifest(&args.registry, name, &chosen.vers).ok();
    let package = manifest.as_ref().and_then(|m| m.get("package"));
    let text = |key: &str| {
        package
            .and_then(|p| p.get(key))
            .and_then(|v| v.as_str())
            .map(str::to_string)
    };
    let keywords: Vec<String> = package
        .and_then(|p| p.get("keywords"))
        .and_then(|k| k.as_array())
        .map(|k| {
            k.iter()
                .filter_map(|v| v.as_str())
                .map(|k| format!("#{k}"))
                .collect()
        })
        .unwrap_or_default();
    let mut out = String::new();
    if keywords.is_empty() {
        out.push_str(&format!("{name}\n"));
    } else {
        out.push_str(&format!("{name} {}\n", keywords.join(" ")));
    }
    if let Some(desc) = text("description") {
        out.push_str(&format!("{}\n", desc.trim()));
    }
    out.push_str(&format!("version: {}", chosen.vers));
    if chosen.yanked {
        out.push_str(" (yanked)");
    }
    out.push('\n');
    for (label, key) in [
        ("license", "license"),
        ("rust-version", "rust-version"),
        ("documentation", "documentation"),
        ("homepage", "homepage"),
        ("repository", "repository"),
    ] {
        if let Some(value) = text(key) {
            out.push_str(&format!("{label}: {value}\n"));
        }
    }
    let crates_io = args
        .registry
        .registry
        .as_deref()
        .is_none_or(|r| r == "crates-io");
    if crates_io {
        out.push_str(&format!(
            "crates.io: https://crates.io/crates/{name}/{}\n",
            chosen.vers
        ));
    }
    let mut features: Vec<(&String, &Vec<String>)> = chosen
        .features
        .iter()
        .chain(chosen.features2.iter())
        .collect();
    features.sort_by(|a, b| a.0.cmp(b.0));
    features.dedup_by(|a, b| a.0 == b.0);
    if !features.is_empty() {
        let mut on: Vec<String> = Vec::new();
        let mut queue = vec!["default".to_string()];
        while let Some(f) = queue.pop() {
            if on.contains(&f) {
                continue;
            }
            on.push(f.clone());
            for (name, subs) in &features {
                if **name == f {
                    for s in subs.iter() {
                        if !s.contains('/') && !s.contains(':') {
                            queue.push(s.clone());
                        }
                    }
                }
            }
        }
        let width = features.iter().map(|(n, _)| n.len()).max().unwrap_or(0);
        out.push_str("features:\n");
        for (fname, subs) in &features {
            let mark = if on.contains(fname) { '+' } else { ' ' };
            out.push_str(&format!(" {mark}{fname:<width$} = [{}]\n", subs.join(", ")));
        }
    }
    let deps: Vec<String> = chosen
        .deps
        .iter()
        .filter(|d| d.kind.as_deref().is_none_or(|k| k == "normal"))
        .map(|d| {
            let shown = d.package.as_deref().unwrap_or(&d.name);
            let mut line = format!("  {shown}@{}", d.req);
            if d.optional {
                line.push_str(" (optional)");
            }
            line
        })
        .collect();
    if !deps.is_empty() {
        out.push_str("dependencies:\n");
        for d in deps {
            out.push_str(&format!("{d}\n"));
        }
    }
    print!("{out}");
    if !args.registry.quiet && locked.is_some() {
        eprintln!(
            "note: to see how you depend on {name}, run `mitts tree --invert --package {name}@{}`",
            chosen.vers
        );
    }
    Ok(())
}

/// The packaged Cargo.toml of `name` `version`, read from its `.crate` download.
fn download_manifest(choice: &RegistryChoice, name: &str, version: &str) -> Result<toml::Value> {
    let config = crate::adapter::rust::registry_config::RegistryConfig::load(&project_dir())?;
    let index = match &choice.index {
        Some(i) => i.trim_start_matches("sparse+").to_string(),
        None => config.index_url_for(choice.registry.as_deref())?,
    };
    let doc = super::registry_api::get_json(
        &format!("{}/config.json", index.trim_end_matches('/')),
        None,
    )?;
    let dl = doc["dl"].as_str().context("the registry has no `dl` URL")?;
    let url = if dl.contains("{crate}") || dl.contains("{version}") {
        dl.replace("{crate}", name).replace("{version}", version)
    } else {
        format!("{}/{name}/{version}/download", dl.trim_end_matches('/'))
    };
    let agent = crate::adapter::rust::http_client::agent_builder().build();
    let response = agent.get(&url).set("User-Agent", "mitts").call()?;
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(64 * 1024 * 1024)
        .read_to_end(&mut bytes)?;
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(&bytes[..]));
    let wanted = format!("{name}-{version}/Cargo.toml");
    for entry in archive.entries()? {
        let mut entry = entry?;
        if entry.path()?.to_string_lossy() == wanted {
            let mut text = String::new();
            entry.read_to_string(&mut text)?;
            return Ok(toml::from_str(&text)?);
        }
    }
    bail!("the package has no Cargo.toml")
}

/// Sets (`Some`) or removes (`None`) a registry's token where Cargo reads it: a new token
/// goes to `credentials.toml` and leaves the legacy `credentials` file, which would
/// otherwise win; a logout removes it from both. A file that cannot be read or parsed is
/// an error, never overwritten.
fn write_credentials(registry: Option<&str>, token: Option<&str>) -> Result<bool> {
    let home = crate::core::source_store::SourceStore::active_cargo_home()
        .context("cannot find the Cargo home directory")?;
    std::fs::create_dir_all(&home)?;
    // Both files are read, parsed and edited in memory first; only then is anything
    // written, and a failure on the second file puts the first one back.
    let mut planned: Vec<(std::path::PathBuf, Option<String>, String)> = Vec::new();
    for (file, write_here) in [("credentials.toml", true), ("credentials", false)] {
        let path = home.join(file);
        let original = match std::fs::read_to_string(&path) {
            Ok(text) => Some(text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if write_here && token.is_some() {
                    None
                } else {
                    continue;
                }
            }
            Err(e) => return Err(e).with_context(|| format!("failed to read {}", path.display())),
        };
        let mut doc: toml_edit::DocumentMut = original
            .as_deref()
            .unwrap_or_default()
            .parse()
            .with_context(|| format!("failed to parse {}", path.display()))?;
        let value = if write_here { token } else { None };
        if edit_token(&mut doc, registry, value)? {
            planned.push((path, original, doc.to_string()));
        }
    }
    let mut written: Vec<(&std::path::PathBuf, &Option<String>)> = Vec::new();
    for (path, original, text) in &planned {
        if let Err(error) = write_private(path, text.as_bytes()) {
            for (done, before) in written {
                match before {
                    Some(before) => {
                        let _ = write_private(done, before.as_bytes());
                    }
                    None => {
                        let _ = std::fs::remove_file(done);
                    }
                }
            }
            return Err(error);
        }
        written.push((path, original));
    }
    Ok(!planned.is_empty())
}

/// Writes a credentials file the way it must be: a new file created readable by the
/// owner only, filled and synced, then renamed over the old one.
fn write_private(path: &std::path::Path, data: &[u8]) -> Result<()> {
    use std::io::Write;
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| -> Result<()> {
        let mut file = options
            .open(&tmp)
            .with_context(|| format!("failed to create {}", tmp.display()))?;
        file.write_all(data)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp, path)
            .with_context(|| format!("failed to replace {}", path.display()))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

fn edit_token(
    doc: &mut toml_edit::DocumentMut,
    registry: Option<&str>,
    token: Option<&str>,
) -> Result<bool> {
    let changed = match registry.filter(|r| *r != "crates-io") {
        None => {
            let table = doc
                .entry("registry")
                .or_insert_with(|| toml_edit::Item::Table(toml_edit::Table::new()))
                .as_table_mut()
                .context("[registry] in credentials.toml is not a table")?;
            match token {
                Some(t) => {
                    table.insert("token", toml_edit::value(t));
                    true
                }
                None => table.remove("token").is_some(),
            }
        }
        Some(name) => {
            let regs = doc
                .entry("registries")
                .or_insert_with(|| {
                    let mut t = toml_edit::Table::new();
                    t.set_implicit(true);
                    toml_edit::Item::Table(t)
                })
                .as_table_mut()
                .context("[registries] in credentials.toml is not a table")?;
            let table = regs
                .entry(name)
                .or_insert_with(|| toml_edit::Item::Table(toml_edit::Table::new()))
                .as_table_mut()
                .context("the registry entry in credentials.toml is not a table")?;
            match token {
                Some(t) => {
                    table.insert("token", toml_edit::value(t));
                    true
                }
                None => table.remove("token").is_some(),
            }
        }
    };
    Ok(changed)
}

pub fn handle_login(args: &LoginArgs) -> i32 {
    report((|| {
        let name = args
            .registry
            .registry
            .clone()
            .unwrap_or_else(|| "crates-io".into());
        let token = match &args.token {
            Some(t) => t.clone(),
            None => {
                if name == "crates-io" {
                    eprintln!("please paste the token found on https://crates.io/me below");
                } else {
                    eprintln!("please paste the token for {name} below");
                }
                let mut line = String::new();
                std::io::stdin().lock().read_line(&mut line)?;
                line
            }
        };
        let token = token.trim();
        if token.is_empty() {
            bail!("please provide a non-empty token");
        }
        if token.contains(char::is_whitespace) || !token.is_ascii() {
            bail!("token contains invalid characters");
        }
        write_credentials(args.registry.registry.as_deref(), Some(token))?;
        if !args.registry.quiet {
            status("Login", &format!("token for `{name}` saved"));
        }
        Ok(())
    })())
}

pub fn handle_logout(args: &LogoutArgs) -> i32 {
    report((|| {
        let name = args
            .registry
            .registry
            .clone()
            .unwrap_or_else(|| "crates-io".into());
        let removed = write_credentials(args.registry.registry.as_deref(), None)?;
        if args.registry.quiet {
            return Ok(());
        }
        if removed {
            status(
                "Logout",
                &format!("token for `{name}` has been removed from local storage"),
            );
            if name == "crates-io" {
                eprintln!(
                    "note: This does not revoke the token on the registry server.\n    \
                     If you need to revoke the token, visit <https://crates.io/me> and follow the instructions there."
                );
            } else {
                eprintln!("note: This does not revoke the token on the registry server.");
            }
        } else {
            status("Logout", &format!("not currently logged in to `{name}`"));
        }
        Ok(())
    })())
}

fn current_package_name() -> Result<String> {
    let manifest = super::project_info::starting_manifest(&None)?;
    crate::adapter::rust::manifest::CargoToml::from_file(&manifest)?
        .package
        .map(|p| p.name)
        .context("a crate name is required: the current manifest has no [package]")
}

fn with_token(api: &mut RegistryApi, token: &Option<String>) {
    if let Some(t) = token {
        api.token_override = Some(t.clone());
    }
}

pub fn handle_owner(args: &OwnerArgs) -> i32 {
    report((|| {
        let krate = match &args.krate {
            Some(k) => k.clone(),
            None => current_package_name()?,
        };
        let mut api = open(&args.registry)?;
        with_token(&mut api, &args.token);
        let path = format!("/crates/{krate}/owners");
        if !args.add.is_empty() {
            if !args.registry.quiet {
                status("Owner", &format!("adding {:?} to crate {krate}", args.add));
            }
            let reply = api.send("PUT", &path, Some(&json!({ "users": args.add })))?;
            print_msg(&reply, args.registry.quiet);
        }
        if !args.remove.is_empty() {
            if !args.registry.quiet {
                status(
                    "Owner",
                    &format!("removing {:?} from crate {krate}", args.remove),
                );
            }
            let reply = api.send("DELETE", &path, Some(&json!({ "users": args.remove })))?;
            print_msg(&reply, args.registry.quiet);
        }
        if args.list || (args.add.is_empty() && args.remove.is_empty()) {
            let token = api.token().ok();
            let doc = api.get(&path, token.as_deref())?;
            for user in doc["users"].as_array().into_iter().flatten() {
                let login = user["login"].as_str().unwrap_or("");
                match user["name"].as_str() {
                    Some(name) => println!("{login} ({name})"),
                    None => println!("{login}"),
                }
            }
        }
        Ok(())
    })())
}

fn print_msg(reply: &Value, quiet: bool) {
    if quiet {
        return;
    }
    if let Some(msg) = reply["msg"].as_str() {
        eprintln!("note: {msg}");
    }
}

pub fn handle_yank(args: &YankArgs) -> i32 {
    report((|| {
        let (krate, version) = match (&args.krate, &args.version) {
            (Some(spec), version) => {
                let (name, from_spec) = super::project_info::parse_spec(spec);
                match (from_spec, version) {
                    (Some(_), Some(_)) => bail!(
                        "cannot specify both `@{}` and `--version`",
                        from_spec.unwrap_or_default()
                    ),
                    (Some(v), None) => (name.to_string(), v.to_string()),
                    (None, Some(v)) => (name.to_string(), v.clone()),
                    (None, None) => bail!("`--version` is required"),
                }
            }
            (None, Some(v)) => (current_package_name()?, v.clone()),
            (None, None) => bail!("`--version` is required"),
        };
        let mut api = open(&args.registry)?;
        with_token(&mut api, &args.token);
        let (word, method, action) = if args.undo {
            ("Unyank", "PUT", "unyank")
        } else {
            ("Yank", "DELETE", "yank")
        };
        if !args.registry.quiet {
            status(word, &format!("{krate}@{version}"));
        }
        api.send(method, &format!("/crates/{krate}/{version}/{action}"), None)?;
        Ok(())
    })())
}
