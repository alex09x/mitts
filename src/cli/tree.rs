/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! `mitts tree`: the dependency graph printed the way `cargo tree` prints it.

use super::tree_graph::{EdgeKind, Graph, Package};
use crate::adapter::rust::types::BuildMode;
use crate::adapter::RustBuildOptions;
use anyhow::{bail, Result};
use clap::Args;
use std::collections::HashSet;
use std::fmt::Write as _;
use std::path::PathBuf;

#[derive(Args, Debug, Clone, Default)]
pub struct TreeArgs {
    /// Package to be used as the root of the tree
    #[arg(short = 'p', long = "package", value_name = "SPEC", action = clap::ArgAction::Append)]
    pub packages: Vec<String>,

    /// Display the tree for all packages in the workspace
    #[arg(long, visible_alias = "all")]
    pub workspace: bool,

    /// Exclude specific workspace members
    #[arg(long, value_name = "SPEC", action = clap::ArgAction::Append)]
    pub exclude: Vec<String>,

    /// The kinds of dependencies to display (normal, build, dev, all, no-normal, no-build,
    /// no-dev, no-proc-macro)
    #[arg(short = 'e', long = "edges", value_name = "KINDS", value_delimiter = ',', action = clap::ArgAction::Append)]
    pub edges: Vec<String>,

    /// Invert the tree direction and focus on the given package
    #[arg(short = 'i', long = "invert", value_name = "SPEC", action = clap::ArgAction::Append)]
    pub invert: Vec<String>,

    /// Prune the given package from the display of the dependency tree
    #[arg(long, value_name = "SPEC", action = clap::ArgAction::Append)]
    pub prune: Vec<String>,

    /// Maximum display depth of the dependency tree
    #[arg(long, value_name = "DEPTH")]
    pub depth: Option<usize>,

    /// Change the prefix (indentation) of how each entry is displayed: indent, depth, none
    #[arg(long, default_value = "indent", value_parser = ["indent", "depth", "none"])]
    pub prefix: String,

    /// Do not de-duplicate (repeats all shared dependencies)
    #[arg(long)]
    pub no_dedupe: bool,

    /// Show only dependencies which come in multiple versions (implies -i)
    #[arg(short = 'd', long)]
    pub duplicates: bool,

    /// Character set to use in output: utf8, ascii
    #[arg(long, default_value = "utf8", value_parser = ["utf8", "ascii"])]
    pub charset: String,

    /// Format string used for printing dependencies ({p}, {l}, {r}, {f}, {lib})
    #[arg(short = 'f', long, default_value = "{p}")]
    pub format: String,

    /// Filter dependencies matching the given target triple
    #[arg(long)]
    pub target: Option<String>,

    /// Space or comma separated list of features to activate
    #[arg(short = 'F', long, value_delimiter = ',', num_args = 1..)]
    pub features: Vec<String>,

    /// Activate all available features
    #[arg(long)]
    pub all_features: bool,

    /// Do not activate the `default` feature
    #[arg(long)]
    pub no_default_features: bool,

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

/// How a tree is drawn.
pub struct Style {
    pub prefix: String,
    pub ascii: bool,
    pub format: String,
    pub depth: Option<usize>,
    pub no_dedupe: bool,
    pub kinds: Vec<EdgeKind>,
    pub hide_proc_macro: bool,
    pub prune: HashSet<String>,
}

pub fn handle_tree(args: &TreeArgs) -> i32 {
    match run(args) {
        Ok(text) => {
            print!("{text}");
            0
        }
        Err(error) => {
            eprintln!("error: {error:#}");
            101
        }
    }
}

fn run(args: &TreeArgs) -> Result<String> {
    let (kinds, hide_proc_macro) = edge_kinds(&args.edges)?;
    let mut opt = RustBuildOptions {
        manifest_path: super::manifest_discovery::discover_manifest(&args.manifest_path),
        features: args.features.clone(),
        no_default_features: args.no_default_features,
        all_features: args.all_features,
        cache_dir: args.cache_dir.clone(),
        target_triple: args.target.clone().filter(|t| t != "all"),
        locked: args.locked || args.frozen,
        offline: args.offline || args.frozen,
        frozen: args.frozen,
        mode: BuildMode::Test,
        all_tests: true,
        all_benches: true,
        all_examples: true,
        all_bins: true,
        ..Default::default()
    };
    if args.workspace {
        opt.extra_args.push("--workspace".to_string());
    }
    opt.exclude = args.exclude.clone();
    let whole = args.workspace || !args.packages.is_empty();
    super::manifest_discovery::select_workspace_member(&mut opt, whole);
    let doc = super::metadata::metadata_for(&opt, None)?;
    let graph = Graph::from_metadata(&doc);

    let mut prune = HashSet::new();
    for spec in &args.prune {
        let found = graph.matching(spec);
        if found.is_empty() {
            bail!("package ID specification `{spec}` did not match any packages");
        }
        prune.extend(found);
    }
    let style = Style {
        prefix: args.prefix.clone(),
        ascii: args.charset == "ascii",
        format: args.format.clone(),
        depth: args.depth,
        no_dedupe: args.no_dedupe,
        kinds,
        hide_proc_macro,
        prune,
    };

    if args.duplicates {
        let inverted = graph.inverted();
        let roots = duplicates(&graph);
        return Ok(render_all(&inverted, &roots, &style));
    }
    if !args.invert.is_empty() {
        let mut roots = Vec::new();
        for spec in &args.invert {
            let found = graph.matching(spec);
            if found.is_empty() {
                bail!("package ID specification `{spec}` did not match any packages");
            }
            roots.extend(found);
        }
        return Ok(render_all(&graph.inverted(), &roots, &style));
    }
    let roots = roots(&graph, args)?;
    Ok(render_all(&graph, &roots, &style))
}

/// `-e`: which edge kinds to follow; `no-proc-macro` hides proc-macro packages.
pub fn edge_kinds(values: &[String]) -> Result<(Vec<EdgeKind>, bool)> {
    let mut kinds: Vec<EdgeKind> = Vec::new();
    let mut removed: Vec<EdgeKind> = Vec::new();
    let mut hide_proc_macro = false;
    for value in values.iter().map(|v| v.trim()) {
        match value {
            "all" => kinds.extend([EdgeKind::Normal, EdgeKind::Build, EdgeKind::Dev]),
            "normal" => kinds.push(EdgeKind::Normal),
            "build" => kinds.push(EdgeKind::Build),
            "dev" => kinds.push(EdgeKind::Dev),
            "no-normal" => removed.push(EdgeKind::Normal),
            "no-build" => removed.push(EdgeKind::Build),
            "no-dev" => removed.push(EdgeKind::Dev),
            "no-proc-macro" => hide_proc_macro = true,
            "features" => bail!(
                "`-e features` is not supported by Mitts; feature edges are listed per \
                 package by `mitts metadata`"
            ),
            other => bail!(
                "unknown edge kind `{other}`, valid values are \"normal\", \"build\", \"dev\", \
                 \"no-normal\", \"no-build\", \"no-dev\", \"no-proc-macro\", \"features\", or \"all\""
            ),
        }
    }
    if kinds.is_empty() {
        kinds = vec![EdgeKind::Normal, EdgeKind::Build, EdgeKind::Dev];
    }
    kinds.retain(|k| !removed.contains(k));
    kinds.sort();
    kinds.dedup();
    Ok((kinds, hide_proc_macro))
}

fn roots(graph: &Graph, args: &TreeArgs) -> Result<Vec<String>> {
    if !args.packages.is_empty() {
        let mut out = Vec::new();
        for spec in &args.packages {
            let found = graph.matching(spec);
            if found.is_empty() {
                bail!("package ID specification `{spec}` did not match any packages");
            }
            out.extend(found);
        }
        return Ok(out);
    }
    if !args.workspace {
        if let Some(root) = &graph.root {
            return Ok(vec![root.clone()]);
        }
    }
    let excluded: HashSet<String> = args.exclude.iter().flat_map(|s| graph.matching(s)).collect();
    let mut members: Vec<String> = graph
        .members
        .iter()
        .filter(|m| !excluded.contains(*m))
        .cloned()
        .collect();
    members.sort_by_key(|id| graph.packages.get(id).map(|p| (p.name.clone(), p.version.clone())));
    Ok(members)
}

/// Packages that appear in more than one version, in name/version order.
fn duplicates(graph: &Graph) -> Vec<String> {
    let mut out: Vec<(String, String, String)> = Vec::new();
    for (id, p) in &graph.packages {
        if graph
            .packages
            .values()
            .filter(|q| q.name == p.name && q.version != p.version)
            .count()
            > 0
        {
            out.push((p.name.clone(), p.version.clone(), id.clone()));
        }
    }
    out.sort();
    out.into_iter().map(|(_, _, id)| id).collect()
}

/// One tree per root, separated by an empty line.
pub fn render_all(graph: &Graph, roots: &[String], style: &Style) -> String {
    let mut out = String::new();
    for (i, root) in roots.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let mut visited = HashSet::new();
        let mut levels = Vec::new();
        let mut stack = Vec::new();
        node(graph, root, style, &mut levels, &mut visited, &mut stack, &mut out);
    }
    out
}

fn node(
    graph: &Graph,
    id: &str,
    style: &Style,
    levels: &mut Vec<bool>,
    visited: &mut HashSet<String>,
    stack: &mut Vec<String>,
    out: &mut String,
) {
    let Some(pkg) = graph.packages.get(id) else {
        return;
    };
    prefix_line(style, levels, out);
    out.push_str(&describe(pkg, &style.format));
    let followed: Vec<EdgeKind> = style.kinds.clone();
    let has_deps = !style.prune.contains(id) && graph.has_edges(id, &followed);
    let in_cycle = stack.iter().any(|s| s == id);
    let new = style.no_dedupe || visited.insert(id.to_string());
    if (!new || in_cycle) && has_deps {
        out.push_str(" (*)");
    }
    out.push('\n');
    if !new || in_cycle || !has_deps || style.depth.is_some_and(|d| levels.len() >= d) {
        return;
    }
    stack.push(id.to_string());
    for kind in [EdgeKind::Normal, EdgeKind::Build, EdgeKind::Dev] {
        if !followed.contains(&kind) {
            continue;
        }
        let children: Vec<&String> = graph
            .edges
            .get(id)
            .into_iter()
            .flatten()
            .filter(|(child, k)| {
                *k == kind
                    && !style.prune.contains(child)
                    && !(style.hide_proc_macro
                        && graph.packages.get(child).is_some_and(|p| p.proc_macro))
            })
            .map(|(child, _)| child)
            .collect();
        if children.is_empty() {
            continue;
        }
        if let Some(header) = kind.header() {
            if style.prefix == "indent" {
                for &more in levels.iter() {
                    out.push_str(continuation(style, more));
                }
                out.push_str(header);
                out.push('\n');
            }
        }
        let count = children.len();
        for (i, child) in children.into_iter().enumerate() {
            levels.push(i + 1 < count);
            node(graph, child, style, levels, visited, stack, out);
            levels.pop();
        }
    }
    stack.pop();
}

fn continuation(style: &Style, more: bool) -> &'static str {
    match (style.ascii, more) {
        (false, true) => "│   ",
        (true, true) => "|   ",
        (_, false) => "    ",
    }
}

fn prefix_line(style: &Style, levels: &[bool], out: &mut String) {
    match style.prefix.as_str() {
        "depth" => {
            let _ = write!(out, "{}", levels.len());
        }
        "none" => {}
        _ => {
            if let Some((last, above)) = levels.split_last() {
                for &more in above {
                    out.push_str(continuation(style, more));
                }
                out.push_str(match (style.ascii, *last) {
                    (false, true) => "├── ",
                    (false, false) => "└── ",
                    (true, true) => "|-- ",
                    (true, false) => "`-- ",
                });
            }
        }
    }
}

/// `{p}` and the other `--format` fields.
pub fn describe(pkg: &Package, format: &str) -> String {
    let mut p = format!("{} v{}", pkg.name, pkg.version);
    match pkg.source.as_deref() {
        None => {
            let _ = write!(p, " ({})", pkg.dir.display());
        }
        Some(s) if s == "registry+https://github.com/rust-lang/crates.io-index" => {}
        Some(s) if s.starts_with("git+") => {
            let _ = write!(p, " ({})", &s[4..]);
        }
        Some(s) => {
            let url = s.split_once('+').map(|(_, u)| u).unwrap_or(s);
            let _ = write!(p, " (registry `{url}`)");
        }
    }
    if pkg.proc_macro {
        p.push_str(" (proc-macro)");
    }
    format
        .replace("{p}", &p)
        .replace("{l}", pkg.license.as_deref().unwrap_or(""))
        .replace("{r}", pkg.repository.as_deref().unwrap_or(""))
        .replace("{f}", &pkg.features.join(","))
        .replace("{lib}", pkg.lib_name.as_deref().unwrap_or(""))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn pkg(name: &str, version: &str, source: Option<&str>) -> Package {
        Package {
            name: name.into(),
            version: version.into(),
            source: source.map(Into::into),
            dir: PathBuf::from(format!("/w/{name}")),
            proc_macro: false,
            workspace: source.is_none(),
            license: None,
            repository: None,
            features: Vec::new(),
            lib_name: None,
        }
    }

    const CRATES_IO: &str = "registry+https://github.com/rust-lang/crates.io-index";

    fn graph() -> Graph {
        let mut packages = BTreeMap::new();
        packages.insert("app".to_string(), pkg("app", "0.1.0", None));
        packages.insert("a".to_string(), pkg("a", "1.0.0", Some(CRATES_IO)));
        packages.insert("b".to_string(), pkg("b", "1.0.0", Some(CRATES_IO)));
        packages.insert("cc".to_string(), pkg("cc", "1.2.0", Some(CRATES_IO)));
        packages.insert("d".to_string(), pkg("d", "0.3.0", Some(CRATES_IO)));
        let mut edges = BTreeMap::new();
        edges.insert(
            "app".to_string(),
            vec![
                ("a".to_string(), EdgeKind::Normal),
                ("b".to_string(), EdgeKind::Normal),
                ("cc".to_string(), EdgeKind::Build),
                ("d".to_string(), EdgeKind::Dev),
            ],
        );
        edges.insert("a".to_string(), vec![("b".to_string(), EdgeKind::Normal)]);
        edges.insert("b".to_string(), vec![("d".to_string(), EdgeKind::Normal)]);
        Graph {
            packages,
            edges,
            root: Some("app".into()),
            members: vec!["app".into()],
        }
    }

    fn style() -> Style {
        Style {
            prefix: "indent".into(),
            ascii: false,
            format: "{p}".into(),
            depth: None,
            no_dedupe: false,
            kinds: vec![EdgeKind::Normal, EdgeKind::Build, EdgeKind::Dev],
            hide_proc_macro: false,
            prune: HashSet::new(),
        }
    }

    #[test]
    fn prints_like_cargo_tree() {
        let text = render_all(&graph(), &["app".to_string()], &style());
        assert_eq!(
            text,
            "app v0.1.0 (/w/app)\n\
             ├── a v1.0.0\n\
             │   └── b v1.0.0\n\
             │       └── d v0.3.0\n\
             └── b v1.0.0 (*)\n\
             [build-dependencies]\n\
             └── cc v1.2.0\n\
             [dev-dependencies]\n\
             └── d v0.3.0\n"
        );
    }

    #[test]
    fn depth_prefix_and_ascii() {
        let mut s = style();
        s.prefix = "depth".into();
        s.depth = Some(1);
        let text = render_all(&graph(), &["app".to_string()], &s);
        assert!(text.starts_with("0app v0.1.0 (/w/app)\n1a v1.0.0\n1b v1.0.0\n"));
        let mut s = style();
        s.ascii = true;
        s.kinds = vec![EdgeKind::Normal];
        let text = render_all(&graph(), &["app".to_string()], &s);
        assert!(text.contains("|-- a v1.0.0\n|   `-- b v1.0.0\n"));
        assert!(!text.contains("cc v1.2.0"));
    }

    #[test]
    fn inverted_tree_lists_dependents() {
        let g = graph();
        let text = render_all(&g.inverted(), &["d".to_string()], &style());
        assert!(
            text.starts_with("d v0.3.0\n└── b v1.0.0\n    ├── a v1.0.0\n"),
            "{text}"
        );
        assert!(text.ends_with("[dev-dependencies]\n└── app v0.1.0 (/w/app)\n"), "{text}");
    }

    #[test]
    fn edge_kinds_parse_like_cargo() {
        let (k, _) = edge_kinds(&["no-dev".into()]).unwrap();
        assert_eq!(k, [EdgeKind::Normal, EdgeKind::Build]);
        let (k, hide) = edge_kinds(&["normal".into(), "no-proc-macro".into()]).unwrap();
        assert_eq!(k, [EdgeKind::Normal]);
        assert!(hide);
        assert!(edge_kinds(&["bogus".into()]).is_err());
    }
}
