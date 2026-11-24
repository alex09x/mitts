/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! `mitts add` and `mitts remove`: Cargo.toml edited in place (comments and layout kept),
//! then Cargo.lock brought up to date, as Cargo does.

use crate::adapter::rust::lockfile::CargoLock;
use crate::adapter::rust::resolver::NativeResolver;
use crate::adapter::rust::sparse_index::SparseIndexEntry;
use anyhow::{bail, Context, Result};
use clap::Args;
use std::path::{Path, PathBuf};
use toml_edit::{Array, DocumentMut, InlineTable, Item, Table, Value};

#[derive(Args, Debug, Clone, Default)]
pub struct AddArgs {
    /// Reference to a package to add as a dependency: NAME or NAME@VERSION_REQ
    #[arg(value_name = "DEP_ID", required_unless_present_any = ["path", "git"])]
    pub crates: Vec<String>,

    /// Mark the dependency as optional
    #[arg(long, conflicts_with = "no_optional")]
    pub optional: bool,

    /// Mark the dependency as required
    #[arg(long)]
    pub no_optional: bool,

    /// Rename the dependency
    #[arg(long, value_name = "NAME")]
    pub rename: Option<String>,

    /// Disable the default features
    #[arg(long, conflicts_with = "default_features")]
    pub no_default_features: bool,

    /// Re-enable the default features
    #[arg(long)]
    pub default_features: bool,

    /// Space or comma separated list of features to activate
    #[arg(short = 'F', long, value_delimiter = ',', num_args = 1..)]
    pub features: Vec<String>,

    /// Filesystem path to local crate to add
    #[arg(long)]
    pub path: Option<PathBuf>,

    /// Git repository location
    #[arg(long)]
    pub git: Option<String>,

    /// Git branch to download the crate from
    #[arg(long, requires = "git")]
    pub branch: Option<String>,

    /// Git tag to download the crate from
    #[arg(long, requires = "git")]
    pub tag: Option<String>,

    /// Git reference to download the crate from
    #[arg(long, requires = "git")]
    pub rev: Option<String>,

    /// Package registry for this dependency
    #[arg(long)]
    pub registry: Option<String>,

    /// Add as development dependency
    #[arg(long, conflicts_with = "build")]
    pub dev: bool,

    /// Add as build dependency
    #[arg(long)]
    pub build: bool,

    /// Add as dependency to the given target platform
    #[arg(long)]
    pub target: Option<String>,

    /// Package to modify
    #[arg(short = 'p', long = "package", value_name = "SPEC")]
    pub package: Option<String>,

    /// Don't actually write the manifest
    #[arg(long)]
    pub dry_run: bool,

    /// The path base to use when adding from a local crate (unstable in Cargo)
    #[arg(long)]
    pub base: Option<String>,

    /// Mark the dependency as public (unstable in Cargo)
    #[arg(long)]
    pub public: bool,

    /// Mark the dependency as private (unstable in Cargo)
    #[arg(long)]
    pub no_public: bool,

    #[command(flatten)]
    pub common: EditCommon,
}

#[derive(Args, Debug, Clone, Default)]
pub struct RemoveArgs {
    /// Dependencies to be removed
    #[arg(value_name = "DEP_ID", required = true)]
    pub crates: Vec<String>,

    /// Remove from development dependencies
    #[arg(long, conflicts_with = "build")]
    pub dev: bool,

    /// Remove from build dependencies
    #[arg(long)]
    pub build: bool,

    /// Remove from target-dependent dependencies
    #[arg(long)]
    pub target: Option<String>,

    /// Package to remove from
    #[arg(short = 'p', long = "package", value_name = "SPEC")]
    pub package: Option<String>,

    /// Don't actually write the manifest
    #[arg(long)]
    pub dry_run: bool,

    #[command(flatten)]
    pub common: EditCommon,
}

#[derive(Args, Debug, Clone, Default)]
pub struct EditCommon {
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

use super::lock_cmds::status;

fn report(result: Result<()>) -> i32 {
    match result {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("error: {error:#}");
            101
        }
    }
}

/// The manifest to edit: `-p` names a workspace member, otherwise the nearest package.
fn target_manifest(common: &EditCommon, package: Option<&str>) -> Result<PathBuf> {
    let start = super::project_info::starting_manifest(&common.manifest_path)?;
    let root = super::project_info::workspace_root(&start)?;
    if let Some(spec) = package {
        let (name, _) = super::project_info::parse_spec(spec);
        let members = super::project_info::local_package_dirs(&root)?;
        return members
            .into_iter()
            .find(|(n, _)| n == name)
            .map(|(_, dir)| dir.join("Cargo.toml"))
            .with_context(|| format!("package(s) `{spec}` not found in workspace"));
    }
    let text = std::fs::read_to_string(&start)?;
    if !text.parse::<DocumentMut>()?.contains_key("package") {
        let members = super::project_info::local_package_dirs(&root)?;
        let names: Vec<String> = members.into_iter().map(|(n, _)| n).collect();
        bail!(
            "`mitts add` could not determine which package to modify. Use the `--package` \
             option to specify a package. \navailable packages: {}",
            names.join(", ")
        );
    }
    Ok(start)
}

fn section_name(dev: bool, build: bool) -> &'static str {
    if dev {
        "dev-dependencies"
    } else if build {
        "build-dependencies"
    } else {
        "dependencies"
    }
}

/// The dependency table, created (after the existing tables) when missing.
fn dep_table<'a>(doc: &'a mut DocumentMut, target: Option<&str>, section: &str) -> &'a mut Table {
    let parent: &mut Table = match target {
        Some(t) => {
            let targets = doc
                .entry("target")
                .or_insert_with(|| {
                    let mut t = Table::new();
                    t.set_implicit(true);
                    Item::Table(t)
                })
                .as_table_mut()
                .expect("[target] is a table");
            let entry = targets.entry(t).or_insert_with(|| {
                let mut t = Table::new();
                t.set_implicit(true);
                Item::Table(t)
            });
            entry.as_table_mut().expect("target entry is a table")
        }
        None => doc.as_table_mut(),
    };
    parent
        .entry(section)
        .or_insert_with(|| Item::Table(Table::new()))
        .as_table_mut()
        .expect("dependency section is a table")
}

pub fn handle_add(args: &AddArgs) -> i32 {
    report(run_add(args))
}

fn run_add(args: &AddArgs) -> Result<()> {
    if args.base.is_some() || args.public || args.no_public {
        bail!("`--base`, `--public` and `--no-public` are unstable in Cargo and not supported by Mitts");
    }
    let manifest = target_manifest(&args.common, args.package.as_deref())?;
    let text = std::fs::read_to_string(&manifest)?;
    let mut doc: DocumentMut = text.parse().context("failed to parse Cargo.toml")?;
    let project_dir = manifest.parent().unwrap_or(Path::new(".")).to_path_buf();
    let root = super::project_info::workspace_root(&manifest)?;
    let workspace_deps = workspace_dependencies(&root)?;
    let section = section_name(args.dev, args.build);
    let offline = args.common.offline || args.common.frozen;
    let mut specs: Vec<(String, Option<String>)> = args
        .crates
        .iter()
        .map(|c| match c.split_once('@') {
            Some((n, v)) => (n.to_string(), Some(v.to_string())),
            None => (c.clone(), None),
        })
        .collect();
    if let Some(path) = &args.path {
        let dep_manifest = project_dir.join(path).join("Cargo.toml");
        let toml = crate::adapter::rust::manifest::CargoToml::from_file(&dep_manifest)
            .with_context(|| format!("failed to read `{}`", dep_manifest.display()))?;
        let name = toml
            .package
            .map(|p| p.name)
            .context("the path has no package")?;
        if specs.is_empty() {
            specs.push((name, None));
        }
    }
    if args.git.is_some() && specs.is_empty() {
        bail!("`--git` needs the package name: `mitts add NAME --git URL`");
    }
    if specs.len() > 1 && (args.rename.is_some() || args.path.is_some() || args.git.is_some()) {
        bail!("cannot specify multiple crates with path, git or `--rename`");
    }
    let resolver = NativeResolver::for_cache_dir(args.common.cache_dir.as_deref())?;
    if args.path.is_none() && args.git.is_none() && !offline && !args.common.quiet {
        status("Updating", "crates.io index");
    }

    for (name, requested) in specs {
        let key = args.rename.clone().unwrap_or_else(|| name.clone());
        let table = dep_table(&mut doc, args.target.as_deref(), section);
        let existing = table.get(&key).cloned();
        // An expanded `[dependencies.foo]` table is edited as it is and written back as a table.
        let was_table = matches!(existing, Some(Item::Table(_)));
        let mut entry = match existing {
            Some(Item::Table(t)) => t.into_inline_table(),
            Some(Item::Value(Value::InlineTable(t))) => t,
            Some(Item::Value(Value::String(s))) => {
                let mut t = InlineTable::new();
                t.insert("version", s.value().as_str().into());
                t
            }
            _ => InlineTable::new(),
        };
        // A new source replaces the old one: path, git and registry fields exclude each other.
        let clear = |entry: &mut InlineTable, keys: &[&str]| {
            for k in keys {
                entry.remove(k);
            }
        };
        let mut shown_version = None;
        let mut available: Option<SparseIndexEntry> = None;
        if workspace_deps.contains(&name)
            && args.path.is_none()
            && args.git.is_none()
            && requested.is_none()
        {
            entry = InlineTable::new();
            entry.insert("workspace", true.into());
        } else if let Some(path) = &args.path {
            clear(
                &mut entry,
                &["version", "git", "branch", "tag", "rev", "registry", "workspace"],
            );
            entry.insert("path", path.to_string_lossy().replace('\\', "/").into());
        } else if let Some(git) = &args.git {
            clear(
                &mut entry,
                &["version", "path", "branch", "tag", "rev", "registry", "workspace"],
            );
            entry.insert("git", git.as_str().into());
            for (k, v) in [
                ("branch", &args.branch),
                ("tag", &args.tag),
                ("rev", &args.rev),
            ] {
                if let Some(v) = v {
                    entry.insert(k, v.as_str().into());
                }
            }
        } else {
            let versions = resolver
                .registry_versions(&project_dir, args.registry.as_deref(), &name, offline)
                .with_context(|| format!("could not find `{name}` in registry"))?;
            let picked =
                pick(&versions, requested.as_deref()).with_context(|| match &requested {
                    Some(r) => format!("could not find `{name}` in registry with version `{r}`"),
                    None => format!("could not find `{name}` in registry"),
                })?;
            let known = |f: &str| {
                picked.features.contains_key(f)
                    || picked.features2.contains_key(f)
                    || picked.deps.iter().any(|d| d.optional && d.name == f)
            };
            let unknown: Vec<&str> = args
                .features
                .iter()
                .flat_map(|f| f.split_whitespace())
                .filter(|f| !f.contains('/') && !known(f))
                .collect();
            if !unknown.is_empty() {
                let mut all: Vec<&String> = picked
                    .features
                    .keys()
                    .chain(picked.features2.keys())
                    .filter(|f| f.as_str() != "default")
                    .collect();
                all.sort();
                all.dedup();
                let available = if all.is_empty() {
                    format!("no features available for crate {name}")
                } else {
                    format!(
                        "available features: {}",
                        all.iter()
                            .map(|s| s.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                };
                bail!(
                    "unrecognized feature for crate {name}: {}\n{available}",
                    unknown.join(", ")
                );
            }
            clear(&mut entry, &["path", "git", "branch", "tag", "rev", "workspace"]);
            let keep_existing = requested.is_none() && entry.contains_key("version");
            if !keep_existing {
                let req = requested.clone().unwrap_or_else(|| picked.vers.clone());
                entry.insert("version", req.as_str().into());
            }
            shown_version = Some(picked.vers.clone());
            available = Some(picked);
            if let Some(registry) = &args.registry {
                entry.insert("registry", registry.as_str().into());
            }
        }
        if args.rename.is_some() {
            entry.insert("package", name.as_str().into());
        }
        if args.no_default_features {
            entry.insert("default-features", false.into());
        } else if args.default_features {
            entry.remove("default-features");
            entry.remove("default_features");
        }
        if args.optional {
            entry.insert("optional", true.into());
        } else if args.no_optional {
            entry.remove("optional");
        }
        if !args.features.is_empty() {
            let mut list: Vec<String> = entry
                .get("features")
                .and_then(|f| f.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            for f in args.features.iter().flat_map(|f| f.split_whitespace()) {
                if !list.iter().any(|x| x == f) {
                    list.push(f.to_string());
                }
            }
            let mut array = Array::new();
            for f in &list {
                array.push(f.as_str());
            }
            entry.insert("features", Value::Array(array));
        }
        if was_table {
            table[key.as_str()] = Item::Table(entry.into_table());
        } else {
            let value = simplify(entry);
            insert_sorted(table, &key, value);
        }

        if !args.common.quiet {
            let place = match &args.target {
                Some(t) => format!("{section} for target `{t}`"),
                None => section.to_string(),
            };
            let opt = if args.optional { "optional " } else { "" };
            let what = match &shown_version {
                Some(v) => format!("{name} v{v}"),
                None if args.path.is_some() => format!("{name} (local)"),
                None if args.git.is_some() => format!("{name} (git)"),
                None => format!("{name} (workspace)"),
            };
            status("Adding", &format!("{what} to {opt}{place}"));
            if let Some(entry) = &available {
                print_features(entry, &args.features, !args.no_default_features);
            }
        }
    }

    if args.dry_run {
        eprintln!("warning: aborting add due to dry run");
        return Ok(());
    }
    std::fs::write(&manifest, doc.to_string())?;
    update_lock(&root, &args.common)
}

/// A plain `name = "req"` when the entry is only a version, as Cargo writes it.
fn simplify(entry: InlineTable) -> Value {
    if entry.len() == 1 {
        if let Some(v) = entry.get("version").and_then(|v| v.as_str()) {
            return v.into();
        }
    }
    let mut entry = entry;
    entry.fmt();
    Value::InlineTable(entry)
}

/// Inserts `key` where a sorted table keeps it sorted, else at the end; an existing key is
/// replaced in place.
fn insert_sorted(table: &mut Table, key: &str, value: Value) {
    if table.contains_key(key) {
        table[key] = Item::Value(value);
        return;
    }
    let keys: Vec<String> = table.iter().map(|(k, _)| k.to_string()).collect();
    let sorted = keys.windows(2).all(|w| w[0] <= w[1]);
    table.insert(key, Item::Value(value));
    if sorted {
        table.sort_values();
    }
}

/// The newest version matching `requested` (any when `None`): a release unless only
/// pre-releases match, never a yanked one.
fn pick(versions: &[SparseIndexEntry], requested: Option<&str>) -> Option<SparseIndexEntry> {
    let req = requested.map(|r| semver::VersionReq::parse(r).ok());
    let mut candidates: Vec<(semver::Version, &SparseIndexEntry)> = versions
        .iter()
        .filter(|e| !e.yanked)
        .filter_map(|e| semver::Version::parse(&e.vers).ok().map(|v| (v, e)))
        .filter(|(v, _)| match &req {
            Some(Some(r)) => r.matches(v),
            Some(None) => false,
            None => true,
        })
        .collect();
    candidates.sort_by(|a, b| a.0.cmp(&b.0));
    candidates
        .iter()
        .rev()
        .find(|(v, _)| v.pre.is_empty())
        .or_else(|| candidates.last())
        .map(|(_, e)| (*e).clone())
}

fn print_features(entry: &SparseIndexEntry, requested: &[String], defaults: bool) {
    let mut all: Vec<&String> = entry
        .features
        .keys()
        .chain(entry.features2.keys())
        .filter(|f| f.as_str() != "default")
        .collect();
    all.sort();
    all.dedup();
    if all.is_empty() {
        return;
    }
    let mut on: Vec<String> = requested.to_vec();
    if defaults {
        let mut queue: Vec<String> = vec!["default".into()];
        while let Some(f) = queue.pop() {
            for sub in entry
                .features
                .get(&f)
                .into_iter()
                .chain(entry.features2.get(&f))
                .flatten()
            {
                if !sub.contains(':') && !sub.contains('/') && !on.contains(sub) {
                    on.push(sub.clone());
                    queue.push(sub.clone());
                }
            }
        }
    }
    eprintln!("{:>12} Features:", "");
    for f in all {
        let mark = if on.contains(f) { '+' } else { '-' };
        eprintln!("{:>12} {mark} {f}", "");
    }
}

fn workspace_dependencies(root: &Path) -> Result<Vec<String>> {
    let text = std::fs::read_to_string(root)?;
    let doc: DocumentMut = text.parse()?;
    Ok(doc
        .get("workspace")
        .and_then(|w| w.get("dependencies"))
        .and_then(|d| d.as_table_like())
        .map(|t| t.iter().map(|(k, _)| k.to_string()).collect())
        .unwrap_or_default())
}

/// Brings Cargo.lock in line with the edited manifest, as Cargo does after `add` and
/// `remove`: pinned versions stay where they still fit, new dependencies are resolved and
/// packages nothing needs any more leave the lock.
fn update_lock(root: &Path, common: &EditCommon) -> Result<()> {
    let lock_path = root.with_file_name("Cargo.lock");
    let previous = CargoLock::from_file(&lock_path).ok();
    let mut resolver = NativeResolver::for_cache_dir(common.cache_dir.as_deref())?;
    if let Some(prev) = &previous {
        resolver = resolver.prefer_locked(prev);
    }
    let mut lock = resolver.resolve_manifest(root, common.offline || common.frozen)?;
    if let Some(prev) = &previous {
        lock.version = lock.version.max(prev.version);
        let changed = !super::lock_cmds::diff(Some(prev), &lock).is_empty()
            || prev.to_string_formatted() != lock.to_string_formatted();
        if !changed {
            return Ok(());
        }
    }
    if common.locked || common.frozen {
        let flag = if common.frozen {
            "--frozen"
        } else {
            "--locked"
        };
        bail!(
            "the lock file {} needs to be updated but {flag} was passed to prevent this",
            lock_path.display()
        );
    }
    lock.write_to_file(&lock_path)
}

pub fn handle_remove(args: &RemoveArgs) -> i32 {
    report(run_remove(args))
}

fn run_remove(args: &RemoveArgs) -> Result<()> {
    let manifest = target_manifest(&args.common, args.package.as_deref())?;
    let text = std::fs::read_to_string(&manifest)?;
    let mut doc: DocumentMut = text.parse().context("failed to parse Cargo.toml")?;
    let section = section_name(args.dev, args.build);
    for name in &args.crates {
        let table = dep_table(&mut doc, args.target.as_deref(), section);
        if table.remove(name).is_none() {
            let place = match &args.target {
                Some(t) => format!("`target.{t}.{section}`"),
                None => format!("`{section}`"),
            };
            bail!("the dependency `{name}` could not be found in {place}.");
        }
        if !args.common.quiet {
            status("Removing", &format!("{name} from {section}"));
        }
        if !still_used(&doc, name) {
            drop_feature_references(&mut doc, name);
        }
    }
    clean_empty_target_tables(&mut doc);
    if args.dry_run {
        eprintln!("warning: aborting remove due to dry run");
        return Ok(());
    }
    std::fs::write(&manifest, doc.to_string())?;
    let root = super::project_info::workspace_root(&manifest)?;
    update_lock(&root, &args.common)
}

/// Whether `name` is still a dependency in another section (then features keep it).
fn still_used(doc: &DocumentMut, name: &str) -> bool {
    let in_table = |t: Option<&Item>| {
        t.and_then(|t| t.as_table_like())
            .is_some_and(|t| t.contains_key(name))
    };
    ["dependencies", "dev-dependencies", "build-dependencies"]
        .iter()
        .any(|s| in_table(doc.get(s)))
        || doc
            .get("target")
            .and_then(|t| t.as_table_like())
            .is_some_and(|targets| {
                targets.iter().any(|(_, spec)| {
                    ["dependencies", "dev-dependencies", "build-dependencies"]
                        .iter()
                        .any(|s| in_table(spec.get(s)))
                })
            })
}

/// Like Cargo: references to a removed dependency (`dep:dep`, `dep/feat`, `dep?/feat`, and
/// a bare `dep` when it named the dependency's implicit feature) go with it. A feature
/// that only enabled the dependency (`dep = ["dep:dep"]`) goes too; an explicit feature
/// of the same name with other contents stays.
fn drop_feature_references(doc: &mut DocumentMut, name: &str) {
    let Some(features) = doc.get_mut("features").and_then(|f| f.as_table_like_mut()) else {
        return;
    };
    let only_enables_dep = features
        .get(name)
        .and_then(|v| v.as_array())
        .is_some_and(|a| a.iter().all(|v| v.as_str() == Some(&format!("dep:{name}"))));
    if only_enables_dep {
        features.remove(name);
    }
    // A bare `name` refers to an explicit feature when one is still defined.
    let explicit_feature = features.contains_key(name);
    let refers = |v: &str| {
        (v == name && !explicit_feature)
            || v == format!("dep:{name}")
            || v.starts_with(&format!("{name}/"))
            || v.starts_with(&format!("{name}?/"))
    };
    for (_, list) in features.iter_mut() {
        if let Some(array) = list.as_array_mut() {
            let before = array.len();
            array.retain(|v| !v.as_str().is_some_and(refers));
            if array.len() != before {
                array.fmt();
            }
        }
    }
}

fn clean_empty_target_tables(doc: &mut DocumentMut) {
    let Some(targets) = doc.get_mut("target").and_then(|t| t.as_table_like_mut()) else {
        return;
    };
    let mut empty_specs = Vec::new();
    for (key, spec) in targets.iter_mut() {
        if let Some(spec) = spec.as_table_like_mut() {
            let empty_sections: Vec<String> = spec
                .iter()
                .filter(|(_, v)| v.as_table_like().is_some_and(|t| t.is_empty()))
                .map(|(k, _)| k.to_string())
                .collect();
            for s in empty_sections {
                spec.remove(&s);
            }
            if spec.is_empty() {
                empty_specs.push(key.to_string());
            }
        }
    }
    for key in empty_specs {
        targets.remove(&key);
    }
    if targets.is_empty() {
        doc.remove("target");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(vers: &str, yanked: bool) -> SparseIndexEntry {
        SparseIndexEntry {
            name: "x".into(),
            vers: vers.into(),
            deps: Vec::new(),
            cksum: String::new(),
            yanked,
            features: Default::default(),
            features2: Default::default(),
        }
    }

    #[test]
    fn picks_the_newest_release_unless_asked() {
        let all = [
            entry("1.0.0", false),
            entry("1.2.0", false),
            entry("1.3.0", true),
            entry("2.0.0-rc.1", false),
        ];
        assert_eq!(pick(&all, None).unwrap().vers, "1.2.0");
        assert_eq!(pick(&all, Some("1.0")).unwrap().vers, "1.2.0");
        assert_eq!(pick(&all, Some("=1.0.0")).unwrap().vers, "1.0.0");
        assert_eq!(pick(&all, Some("2.0.0-rc.1")).unwrap().vers, "2.0.0-rc.1");
        assert!(pick(&all, Some("3")).is_none());
    }

    #[test]
    fn edits_keep_layout_and_sorting() {
        let mut doc: DocumentMut =
            "[package]\nname = \"a\"\n\n# deps\n[dependencies]\nanyhow = \"1\"\nzip = \"2\"\n"
                .parse()
                .unwrap();
        let table = dep_table(&mut doc, None, "dependencies");
        insert_sorted(table, "serde", "1.0.200".into());
        let text = doc.to_string();
        assert!(
            text.contains(
                "# deps\n[dependencies]\nanyhow = \"1\"\nserde = \"1.0.200\"\nzip = \"2\"\n"
            ),
            "{text}"
        );
        let table = dep_table(&mut doc, Some("cfg(unix)"), "dev-dependencies");
        insert_sorted(table, "libc", "0.2".into());
        assert!(
            doc.to_string()
                .contains("[target.'cfg(unix)'.dev-dependencies]\nlibc = \"0.2\"\n")
                || doc
                    .to_string()
                    .contains("[target.\"cfg(unix)\".dev-dependencies]\nlibc = \"0.2\"\n"),
            "{}",
            doc
        );
    }

    #[test]
    fn removing_drops_feature_references() {
        let mut doc: DocumentMut = "[dependencies]\nserde = \"1\"\n\n[features]\nfull = [\"serde/derive\", \"dep:serde\", \"other\"]\nserde = [\"dep:serde\"]\n"
            .parse()
            .unwrap();
        dep_table(&mut doc, None, "dependencies").remove("serde");
        assert!(!still_used(&doc, "serde"));
        drop_feature_references(&mut doc, "serde");
        let text = doc.to_string();
        assert!(text.contains("full = [\"other\"]"), "{text}");
        assert!(!text.contains("\nserde ="), "{text}");
    }
}
