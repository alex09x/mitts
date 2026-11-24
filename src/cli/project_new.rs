/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! `mitts new` and `mitts init`, with Cargo's files, name rules, version control and
//! workspace membership.

use anyhow::{bail, Context, Result};
use clap::Args;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Args, Debug, Clone, Default)]
pub struct NewArgs {
    /// Directory of the new package
    #[arg(value_name = "PATH")]
    pub path: Option<PathBuf>,

    /// Use a binary (application) template [default]
    #[arg(long, conflicts_with = "lib")]
    pub bin: bool,

    /// Use a library template
    #[arg(long)]
    pub lib: bool,

    /// Set the resulting package name, defaults to the directory name
    #[arg(long)]
    pub name: Option<String>,

    /// Edition to set for the package generated
    #[arg(long, value_parser = ["2015", "2018", "2021", "2024"])]
    pub edition: Option<String>,

    /// Initialize a new repository for the given version control system: git or none
    #[arg(long, value_parser = ["git", "hg", "pijul", "fossil", "none"])]
    pub vcs: Option<String>,

    /// Registry to use (recorded as `package.publish`)
    #[arg(long)]
    pub registry: Option<String>,

    /// Use verbose output
    #[arg(short, long, action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// Do not print Mitts log messages
    #[arg(short, long)]
    pub quiet: bool,
}

pub fn handle_new(args: &NewArgs) -> i32 {
    report(run(args, false))
}

pub fn handle_init(args: &NewArgs) -> i32 {
    report(run(args, true))
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

fn run(args: &NewArgs, init: bool) -> Result<()> {
    let cwd = std::env::current_dir().context("current directory")?;
    let dir = match (&args.path, init) {
        (Some(p), _) => cwd.join(p),
        (None, true) => cwd.clone(),
        (None, false) => bail!("the following required arguments were not provided:\n  <PATH>"),
    };
    if !init && dir.exists() {
        bail!(
            "destination `{}` already exists\n\nUse `mitts init` to initialize the directory",
            dir.display()
        );
    }
    if init && dir.join("Cargo.toml").exists() {
        bail!("`mitts init` cannot be run on existing Cargo packages");
    }
    let dir_name = dir
        .file_name()
        .and_then(|n| n.to_str())
        .context("the package directory has no usable name")?
        .to_string();
    let name = args.name.clone().unwrap_or_else(|| dir_name.clone());
    // Existing sources decide the kind on `init`, as Cargo does.
    let found = if init {
        existing_targets(&dir)
    } else {
        Found::default()
    };
    let lib = args.lib || (!args.bin && found.lib.is_some() && found.bin.is_none());
    let bin = !lib || found.bin.is_some();
    check_name(&name, bin, args.name.is_none())?;

    fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    let edition = args.edition.clone().unwrap_or_else(default_edition);
    let vcs = choose_vcs(args.vcs.as_deref(), &dir, init);
    match vcs {
        "git" => init_git(&dir)?,
        "hg" | "pijul" | "fossil" => init_other_vcs(vcs, &dir)?,
        _ => {}
    }

    let mut manifest =
        format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"{edition}\"\n");
    if let Some(registry) = &args.registry {
        manifest.push_str(&format!("publish = [\"{registry}\"]\n"));
    }
    if let Some(path) = &found.lib {
        if path != Path::new("src/lib.rs") {
            manifest.push_str(&format!("\n[lib]\npath = \"{}\"\n", path.display()));
        }
    }
    if let Some(path) = &found.bin {
        if path != Path::new("src/main.rs") {
            manifest.push_str(&format!(
                "\n[[bin]]\nname = \"{name}\"\npath = \"{}\"\n",
                path.display()
            ));
        }
    }
    manifest.push_str("\n[dependencies]\n");
    fs::write(dir.join("Cargo.toml"), manifest)?;

    if found.lib.is_none() && found.bin.is_none() {
        fs::create_dir_all(dir.join("src"))?;
        if lib {
            fs::write(dir.join("src/lib.rs"), LIB_TEMPLATE)?;
        } else {
            fs::write(dir.join("src/main.rs"), MAIN_TEMPLATE)?;
        }
    }

    let kind = if lib {
        "library"
    } else {
        "binary (application)"
    };
    if !args.quiet {
        if init {
            eprintln!("    Creating {kind} package");
        } else {
            eprintln!("    Creating {kind} `{name}` package");
        }
    }
    if let Some(root) = join_enclosing_workspace(&dir)? {
        if !args.quiet {
            eprintln!(
                "      Adding `{name}` as member of workspace at `{}`",
                root.display()
            );
        }
    }
    if !args.quiet {
        eprintln!(
            "note: see more `Cargo.toml` keys and their definitions at \
             https://doc.rust-lang.org/cargo/reference/manifest.html"
        );
    }
    Ok(())
}

const MAIN_TEMPLATE: &str = "fn main() {\n    println!(\"Hello, world!\");\n}\n";

const LIB_TEMPLATE: &str = "pub fn add(left: u64, right: u64) -> u64 {
    left + right
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_works() {
        let result = add(2, 2);
        assert_eq!(result, 4);
    }
}
";

#[derive(Default)]
struct Found {
    lib: Option<PathBuf>,
    bin: Option<PathBuf>,
}

/// Sources `init` adopts instead of writing a template: src/ first, then the directory.
fn existing_targets(dir: &Path) -> Found {
    let pick = |names: [&str; 2]| {
        names
            .into_iter()
            .map(PathBuf::from)
            .find(|p| dir.join(p).is_file())
    };
    Found {
        lib: pick(["src/lib.rs", "lib.rs"]),
        bin: pick(["src/main.rs", "main.rs"]),
    }
}

const KEYWORDS: &[&str] = &[
    "abstract", "as", "async", "await", "become", "box", "break", "const", "continue", "crate",
    "do", "dyn", "else", "enum", "extern", "false", "final", "fn", "for", "gen", "if", "impl",
    "in", "let", "loop", "macro", "match", "mod", "move", "mut", "override", "priv", "pub", "ref",
    "return", "self", "Self", "static", "struct", "super", "trait", "true", "try", "type",
    "typeof", "unsafe", "unsized", "use", "virtual", "where", "while", "yield",
];

/// Cargo's package-name rules for `new` / `init`.
pub fn check_name(name: &str, bin: bool, from_dir: bool) -> Result<()> {
    let hint = if from_dir {
        "\nIf you need a package name to not match the directory name, consider using --name flag."
    } else {
        ""
    };
    if name.is_empty() {
        bail!("package name cannot be empty");
    }
    if let Some(c) = name
        .chars()
        .find(|c| !(c.is_alphanumeric() || *c == '-' || *c == '_'))
    {
        bail!(
            "invalid character `{c}` in package name: `{name}`, characters must be Unicode XID \
             characters (numbers, `-`, `_`, or most letters){hint}"
        );
    }
    if name.starts_with(|c: char| c.is_ascii_digit()) {
        bail!(
            "invalid character `{}` in package name: `{name}`, the name cannot start with a \
             digit{hint}",
            &name[..1]
        );
    }
    if KEYWORDS.contains(&name) {
        bail!("the name `{name}` cannot be used as a package name, it is a Rust keyword{hint}");
    }
    if name == "test" {
        bail!(
            "the name `test` cannot be used as a package name, it conflicts with Rust's \
             built-in test library{hint}"
        );
    }
    if ["core", "std", "alloc", "proc_macro", "proc-macro"].contains(&name) {
        bail!(
            "the name `{name}` cannot be used as a package name, it conflicts with Rust's \
             built-in library{hint}"
        );
    }
    if bin && ["deps", "examples", "build", "incremental"].contains(&name) {
        bail!(
            "the name `{name}` cannot be used as a package name, it conflicts with cargo's \
             build directory names{hint}"
        );
    }
    Ok(())
}

/// Cargo's default edition: the newest one the rustc in use supports.
fn default_edition() -> String {
    let version = crate::adapter::RustAdapter::detect_toolchain()
        .ok()
        .and_then(|t| crate::adapter::rust::rust_version::parse(&t.version));
    match version {
        Some(v) if v >= (1, 85, 0) => "2024",
        Some(v) if v < (1, 56, 0) => "2018",
        _ => "2021",
    }
    .to_string()
}

/// `--vcs`, else `cargo-new.vcs`, else git unless the directory is already inside a git
/// work tree, as Cargo does.
fn choose_vcs(flag: Option<&str>, dir: &Path, init: bool) -> &'static str {
    let cwd = std::env::current_dir().unwrap_or_else(|_| dir.to_path_buf());
    let configured = crate::adapter::rust::cargo_config_value::get(&cwd, "cargo-new.vcs")
        .ok()
        .flatten()
        .and_then(|v| v.as_str().ok().map(str::to_string));
    let flag = flag.or(configured.as_deref());
    match flag {
        Some("none") => "none",
        Some("git") => "git",
        Some("hg") => "hg",
        Some("pijul") => "pijul",
        Some("fossil") => "fossil",
        Some(other) => {
            eprintln!("warning: unknown version control `{other}`; no repository created");
            "none"
        }
        None => {
            if init && dir.join(".git").exists() {
                return "git";
            }
            let probe = dir.parent().unwrap_or(dir);
            let inside = Command::new("git")
                .args(["rev-parse", "--is-inside-work-tree"])
                .current_dir(probe)
                .output()
                .is_ok_and(|o| o.status.success());
            if inside {
                "none"
            } else {
                "git"
            }
        }
    }
}

fn init_git(dir: &Path) -> Result<()> {
    if !dir.join(".git").exists() {
        let status = Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(dir)
            .status();
        if !status.is_ok_and(|s| s.success()) {
            eprintln!(
                "warning: `git init` failed in {}; no repository created",
                dir.display()
            );
            return Ok(());
        }
    }
    let ignore = dir.join(".gitignore");
    let existing = fs::read_to_string(&ignore).unwrap_or_default();
    if !existing.lines().any(|l| l.trim() == "/target") {
        let mut text = existing;
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str("/target\n");
        fs::write(&ignore, text)?;
    }
    Ok(())
}

/// Mercurial, Pijul or Fossil as Cargo sets them up: the repository (a failing tool is an
/// error, as in Cargo) and the tool's ignore file naming `target`.
fn init_other_vcs(vcs: &str, dir: &Path) -> Result<()> {
    let run = |program: &str, args: &[&std::ffi::OsStr], cwd: &Path| -> Result<()> {
        let status = Command::new(program)
            .args(args)
            .current_dir(cwd)
            .status()
            .with_context(|| format!("could not run `{program}`; is it installed?"))?;
        if !status.success() {
            anyhow::bail!(
                "`{program} {}` failed in {}",
                args.iter()
                    .map(|a| a.to_string_lossy())
                    .collect::<Vec<_>>()
                    .join(" "),
                cwd.display()
            );
        }
        Ok(())
    };
    let (ignore, line) = match vcs {
        "hg" => {
            if !dir.join(".hg").exists() {
                run("hg", &["init".as_ref(), dir.as_os_str()], dir)?;
            }
            let ignore = dir.join(".hgignore");
            // `^target$` is a regexp: when the file's last `syntax:` line switched to glob,
            // switch back before adding it.
            let existing = read_existing(&ignore)?;
            let glob_last = existing
                .lines()
                .rev()
                .find_map(|l| l.trim().strip_prefix("syntax:"))
                .is_some_and(|s| s.trim() != "regexp");
            if glob_last && !existing.lines().any(|l| l.trim() == "^target$") {
                append_line(&ignore, "syntax: regexp")?;
            }
            (ignore, "^target$")
        }
        "pijul" => {
            if !dir.join(".pijul").exists() {
                run("pijul", &["init".as_ref()], dir)?;
            }
            (dir.join(".ignore"), "/target")
        }
        _ => {
            let db = dir.join(".fossil");
            if !db.exists() {
                run(
                    "fossil",
                    &["init".as_ref(), "--empty".as_ref(), db.as_os_str()],
                    dir,
                )?;
            }
            // The checkout is separate from the repository: a run whose `open` failed opens
            // it now. `--force` opens into a directory that already has files (`init`).
            let checked_out = dir.join(".fslckout").exists() || dir.join("_FOSSIL_").exists();
            if !checked_out {
                run(
                    "fossil",
                    &["open".as_ref(), "--force".as_ref(), ".fossil".as_ref()],
                    dir,
                )?;
            }
            let settings = dir.join(".fossil-settings");
            fs::create_dir_all(&settings)?;
            append_line(&settings.join("clean-glob"), "target")?;
            (settings.join("ignore-glob"), "target")
        }
    };
    append_line(&ignore, line)
}

/// The file's text; a missing file is empty, any other failure (unreadable, not UTF-8) is
/// an error, so the file is never rewritten from less than it holds.
fn read_existing(path: &Path) -> Result<String> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e).with_context(|| format!("failed to read {}", path.display())),
    }
}

/// Adds `line` to the file unless one of its lines already is `line`.
fn append_line(path: &Path, line: &str) -> Result<()> {
    let existing = read_existing(path)?;
    if existing.lines().any(|l| l.trim() == line) {
        return Ok(());
    }
    let mut text = existing;
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(line);
    text.push('\n');
    fs::write(path, text).with_context(|| format!("write {}", path.display()))
}

/// Like Cargo: a package created inside a workspace whose `members` do not cover it is
/// added to them. Returns the workspace root when the manifest was changed.
fn join_enclosing_workspace(dir: &Path) -> Result<Option<PathBuf>> {
    for root in dir.ancestors().skip(1) {
        let manifest = root.join("Cargo.toml");
        let Ok(text) = fs::read_to_string(&manifest) else {
            continue;
        };
        let Ok(mut doc) = text.parse::<toml_edit::DocumentMut>() else {
            continue;
        };
        let Some(ws) = doc.get_mut("workspace").and_then(|w| w.as_table_like_mut()) else {
            continue;
        };
        let relative = dir
            .strip_prefix(root)
            .unwrap_or(dir)
            .to_string_lossy()
            .replace('\\', "/");
        let listed = |key: &str, ws: &dyn toml_edit::TableLike| -> bool {
            ws.get(key).and_then(|m| m.as_array()).is_some_and(|a| {
                a.iter()
                    .filter_map(|v| v.as_str())
                    .any(|p| glob_match(p, &relative))
            })
        };
        if listed("members", ws) || listed("exclude", ws) {
            return Ok(None);
        }
        let Some(members) = ws.get_mut("members").and_then(|m| m.as_array_mut()) else {
            return Ok(None);
        };
        members.push(relative.as_str());
        fs::write(&manifest, doc.to_string())?;
        return Ok(Some(root.to_path_buf()));
    }
    Ok(None)
}

/// `members` glob: `*` and `?` within one path component, compared component by component.
pub fn glob_match(pattern: &str, path: &str) -> bool {
    let pattern = pattern.trim_end_matches('/');
    let p: Vec<&str> = pattern
        .split('/')
        .filter(|s| !s.is_empty() && *s != ".")
        .collect();
    let s: Vec<&str> = path
        .split('/')
        .filter(|s| !s.is_empty() && *s != ".")
        .collect();
    p.len() == s.len()
        && p.iter()
            .zip(&s)
            .all(|(p, s)| component_match(p.as_bytes(), s.as_bytes()))
}

fn component_match(p: &[u8], s: &[u8]) -> bool {
    match (p.first(), s.first()) {
        (None, None) => true,
        (Some(b'*'), _) => {
            component_match(&p[1..], s) || (!s.is_empty() && component_match(p, &s[1..]))
        }
        (Some(b'?'), Some(_)) => component_match(&p[1..], &s[1..]),
        (Some(a), Some(b)) if a == b => component_match(&p[1..], &s[1..]),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_follow_cargos_rules() {
        assert!(check_name("hello-world_2", true, true).is_ok());
        assert!(check_name("2fast", true, true).is_err());
        assert!(check_name("fn", true, true).is_err());
        assert!(check_name("test", false, true).is_err());
        assert!(check_name("std", true, true).is_err());
        assert!(check_name("deps", true, true).is_err());
        assert!(check_name("deps", false, true).is_ok());
        assert!(check_name("a.b", true, true).is_err());
    }

    #[test]
    fn member_globs_match_one_component_per_star() {
        assert!(glob_match("crates/*", "crates/foo"));
        assert!(!glob_match("crates/*", "crates/foo/bar"));
        assert!(glob_match("foo", "foo"));
        assert!(glob_match("./foo/", "foo"));
        assert!(glob_match("c?t", "cat"));
        assert!(!glob_match("crates/*", "other/foo"));
    }
}
