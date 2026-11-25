/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Differential tests: Cargo is the reference. The same fixture is copied twice; one copy
//! goes through Cargo, the other through Mitts, and what a user sees must agree: the
//! lockfile, `metadata`, `tree`, `pkgid`, `locate-project`, the artifacts a build leaves,
//! `run` output, test results, `new`/`init` trees, `add`/`remove` edits, `package --list`.
//! Cargo runs here only as the oracle (owner decision 2026-11-25, AGENTS.md); without it on
//! PATH the tests say so and stop, except under CI where it must be present.

use serde_json::Value;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;

/// The reference Cargo: `REFERENCE_CARGO` (no `MITTS_` prefix: no `MITTS_*` variable may
/// reach the suite), which the isolated runner sets to the real
/// Cargo it hides behind its guard for every other test, else `cargo` from PATH.
fn cargo() -> String {
    std::env::var("REFERENCE_CARGO")
        .ok()
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| "cargo".to_string())
}

fn cargo_available() -> bool {
    let ok = Command::new(cargo())
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    if !ok {
        assert!(
            std::env::var_os("CI").is_none(),
            "CI must provide cargo as the reference"
        );
        eprintln!("skipped: cargo is not on PATH");
    }
    ok
}

fn write(path: impl AsRef<Path>, contents: &str) {
    let path = path.as_ref();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// A workspace with what a real one has: a binary + library package with an example, an
/// integration test, a build script, features and a target-specific dependency; a path
/// library; a dev-dependency.
fn fixture(root: &Path) {
    write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"app\", \"util\"]\nresolver = \"2\"\n\n\
         [workspace.package]\nversion = \"0.3.0\"\nedition = \"2021\"\nlicense = \"MIT\"\n",
    );
    write(
        root.join("app/Cargo.toml"),
        "[package]\nname = \"app\"\nversion.workspace = true\nedition.workspace = true\n\
         license.workspace = true\ndescription = \"An app\"\n\n\
         [features]\ndefault = [\"fast\"]\nfast = []\nextra = [\"util/loud\"]\n\n\
         [dependencies]\nutil = { path = \"../util\" }\n\n\
         [target.'cfg(windows)'.dependencies]\nwinonly = { path = \"../winonly\" }\n\n\
         [dev-dependencies]\ntesthelp = { path = \"../testhelp\" }\n",
    );
    write(
        root.join("app/build.rs"),
        "fn main() { println!(\"cargo:rustc-env=APP_BUILT=yes\"); }\n",
    );
    write(
        root.join("app/src/lib.rs"),
        "pub fn answer() -> u32 { util::base() * 2 }\n\
         #[cfg(test)]\nmod tests { #[test] fn unit() { assert_eq!(super::answer(), 42); } }\n",
    );
    write(
        root.join("app/src/main.rs"),
        "fn main() { println!(\"answer {} built {}\", app::answer(), env!(\"APP_BUILT\")); }\n",
    );
    write(
        root.join("app/examples/demo.rs"),
        "fn main() { println!(\"demo {}\", app::answer()); }\n",
    );
    write(
        root.join("app/tests/integration.rs"),
        "#[test] fn integrates() { assert_eq!(app::answer(), testhelp::expected()); }\n\
         #[test] fn twice() { assert_eq!(app::answer() + 0, 42); }\n",
    );
    write(
        root.join("util/Cargo.toml"),
        "[package]\nname = \"util\"\nversion = \"1.2.0\"\nedition = \"2021\"\nlicense = \"MIT\"\n\n\
         [features]\nloud = []\n",
    );
    write(
        root.join("util/src/lib.rs"),
        "pub fn base() -> u32 { 21 }\n",
    );
    for (name, body) in [
        ("winonly", "pub fn w() {}\n"),
        ("testhelp", "pub fn expected() -> u32 { 42 }\n"),
    ] {
        write(
            root.join(name).join("Cargo.toml"),
            &format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
        );
        write(root.join(name).join("src/lib.rs"), body);
    }
}

struct Pair {
    _temp: TempDir,
    cargo_root: PathBuf,
    mitts_root: PathBuf,
    home: PathBuf,
}

impl Pair {
    fn new(build: impl Fn(&Path)) -> Self {
        let temp = TempDir::new().unwrap();
        let cargo_root = temp.path().join("c").join("ws");
        let mitts_root = temp.path().join("m").join("ws");
        build(&cargo_root);
        build(&mitts_root);
        let home = temp.path().join("cargo-home");
        fs::create_dir_all(&home).unwrap();
        Pair {
            _temp: temp,
            cargo_root,
            mitts_root,
            home,
        }
    }

    fn run(&self, program: &str, dir: &Path, args: &[&str]) -> Output {
        let mut cmd = Command::new(program);
        cmd.args(args)
            .current_dir(dir)
            .env("CARGO_HOME", &self.home);
        for name in [
            "CARGO_TARGET_DIR",
            "CARGO_BUILD_TARGET",
            "RUSTFLAGS",
            "CARGO_ENCODED_RUSTFLAGS",
            "RUSTC_WRAPPER",
            "RUSTC_WORKSPACE_WRAPPER",
            "MITTS_CACHE_DIR",
            "CARGO_TERM_COLOR",
        ] {
            cmd.env_remove(name);
        }
        if program != cargo() {
            cmd.env(
                "MITTS_CACHE_DIR",
                self.home.parent().unwrap().join("mitts-cache"),
            );
        }
        cmd.output().unwrap()
    }

    /// The same command through both, from the same relative directory.
    fn both(&self, rel: &str, args: &[&str]) -> (Output, Output) {
        let c = self.run(&cargo(), &self.cargo_root.join(rel), args);
        let m = self.run(
            env!("CARGO_BIN_EXE_mitts"),
            &self.mitts_root.join(rel),
            args,
        );
        (c, m)
    }
}

fn text(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn err(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

/// Paths and ids made comparable: each side's root becomes `<ws>`.
/// On Windows the root also appears without canonicalize's `\\?\` prefix and with forward
/// slashes (`path+file:///C:/...` ids), so every spelling is replaced, longest first.
fn norm(s: &str, root: &Path) -> String {
    let canonical = root.canonicalize().unwrap_or(root.to_path_buf());
    let mut forms = Vec::new();
    for path in [canonical.display().to_string(), root.display().to_string()] {
        let plain = path.strip_prefix(r"\\?\").unwrap_or(&path).to_string();
        forms.push(plain.replace('\\', "/"));
        forms.push(plain);
        forms.push(path);
    }
    forms.sort_by_key(|f| std::cmp::Reverse(f.len()));
    forms.dedup();
    let mut out = s.to_string();
    for form in &forms {
        out = out.replace(form.as_str(), "<ws>");
    }
    out.replace('\\', "/")
}

fn agree(what: &str, c: &Output, m: &Output, pair: &Pair) {
    assert_eq!(
        c.status.success(),
        m.status.success(),
        "{what}: success differs\ncargo: {}\nmitts: {}",
        err(c),
        err(m)
    );
    assert_eq!(
        norm(&text(c), &pair.cargo_root),
        norm(&text(m), &pair.mitts_root),
        "{what}: stdout differs (left cargo, right mitts)\nmitts stderr: {}",
        err(m)
    );
}

#[test]
fn lockfile_metadata_tree_and_ids_agree_with_cargo() {
    if !cargo_available() {
        return;
    }
    let pair = Pair::new(fixture);

    let (c, m) = pair.both(".", &["generate-lockfile", "--offline"]);
    assert!(
        c.status.success() && m.status.success(),
        "{}{}",
        err(&c),
        err(&m)
    );
    assert_eq!(
        fs::read_to_string(pair.cargo_root.join("Cargo.lock")).unwrap(),
        fs::read_to_string(pair.mitts_root.join("Cargo.lock")).unwrap(),
        "Cargo.lock"
    );

    for args in [
        &["tree", "--offline"][..],
        &["tree", "--offline", "-e", "features"],
        &["tree", "--offline", "--target", "x86_64-pc-windows-msvc"],
        &["tree", "--offline", "-i", "util"],
        &["tree", "--offline", "-p", "app", "--features", "extra"],
        &["pkgid", "-p", "util", "--offline"],
        &["locate-project"],
        &["locate-project", "--workspace"],
    ] {
        let (c, m) = pair.both("app", args);
        agree(&args.join(" "), &c, &m, &pair);
    }

    let (c, m) = pair.both(".", &["metadata", "--format-version", "1", "--offline"]);
    assert!(
        c.status.success() && m.status.success(),
        "{}{}",
        err(&c),
        err(&m)
    );
    let cj: Value = serde_json::from_str(&norm(&text(&c), &pair.cargo_root)).unwrap();
    let mj: Value = serde_json::from_str(&norm(&text(&m), &pair.mitts_root)).unwrap();
    assert_eq!(
        cj["workspace_members"], mj["workspace_members"],
        "workspace_members"
    );
    assert_eq!(cj["workspace_root"], mj["workspace_root"], "workspace_root");
    let packages = |v: &Value| -> Vec<Value> {
        let mut out: Vec<Value> = v["packages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| {
                let mut targets: Vec<Value> = p["targets"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|t| {
                        serde_json::json!({
                            "name": t["name"], "kind": t["kind"],
                            "crate_types": t["crate_types"], "src_path": t["src_path"],
                            "test": t["test"], "doctest": t["doctest"]
                        })
                    })
                    .collect();
                targets.sort_by_key(|t| t.to_string());
                let mut deps: Vec<Value> = p["dependencies"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|d| {
                        serde_json::json!({
                            "name": d["name"], "kind": d["kind"], "target": d["target"],
                            "optional": d["optional"], "req": d["req"], "path": d["path"]
                        })
                    })
                    .collect();
                deps.sort_by_key(|d| d.to_string());
                serde_json::json!({
                    "id": p["id"], "name": p["name"], "version": p["version"],
                    "license": p["license"], "description": p["description"],
                    "features": p["features"], "manifest_path": p["manifest_path"],
                    "edition": p["edition"], "targets": targets, "dependencies": deps
                })
            })
            .collect();
        out.sort_by_key(|p| p["name"].to_string());
        out
    };
    assert_eq!(packages(&cj), packages(&mj), "metadata packages");
    let nodes = |v: &Value| -> Vec<Value> {
        let mut out: Vec<Value> = v["resolve"]["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| {
                let mut deps: Vec<Value> = n["deps"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|d| serde_json::json!({"name": d["name"], "pkg": d["pkg"], "dep_kinds": d["dep_kinds"]}))
                    .collect();
                deps.sort_by_key(|d| d.to_string());
                serde_json::json!({"id": n["id"], "features": n["features"], "deps": deps})
            })
            .collect();
        out.sort_by_key(|n| n["id"].to_string());
        out
    };
    assert_eq!(nodes(&cj), nodes(&mj), "metadata resolve nodes");
}

/// The top-level files a build leaves for the user (Cargo's own bookkeeping excluded).
fn artifacts(dir: &Path) -> BTreeSet<String> {
    fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| !n.starts_with('.') && !n.ends_with(".d") && !n.ends_with(".pdb"))
        .collect()
}

#[test]
fn builds_runs_and_tests_agree_with_cargo() {
    if !cargo_available() {
        return;
    }
    let pair = Pair::new(fixture);
    let (c, m) = pair.both(".", &["build", "--offline", "--workspace", "--all-targets"]);
    assert!(c.status.success(), "cargo: {}", err(&c));
    assert!(m.status.success(), "mitts: {}", err(&m));
    assert_eq!(
        artifacts(&pair.cargo_root.join("target/debug")),
        artifacts(&pair.mitts_root.join("target/debug")),
        "target/debug"
    );
    assert_eq!(
        artifacts(&pair.cargo_root.join("target/debug/examples"))
            .into_iter()
            .filter(|n| !n.contains('-'))
            .collect::<BTreeSet<_>>(),
        artifacts(&pair.mitts_root.join("target/debug/examples"))
            .into_iter()
            .filter(|n| !n.contains('-'))
            .collect::<BTreeSet<_>>(),
        "target/debug/examples"
    );

    for args in [
        &["run", "--offline", "-q", "-p", "app"][..],
        &["run", "--offline", "-q", "-p", "app", "--example", "demo"],
    ] {
        let (c, m) = pair.both(".", args);
        agree(&args.join(" "), &c, &m, &pair);
    }

    // Test results: the same tests, the same outcome, in the same order.
    let (c, m) = pair.both(".", &["test", "--offline", "--workspace"]);
    assert_eq!(
        c.status.success(),
        m.status.success(),
        "{}\n{}",
        err(&c),
        err(&m)
    );
    let results = |o: &Output| -> Vec<String> {
        text(o)
            .lines()
            .filter(|l| l.starts_with("test ") && (l.ends_with(" ok") || l.contains("FAILED")))
            .map(str::to_string)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    };
    assert_eq!(results(&c), results(&m), "test results");
}

#[test]
fn new_init_add_remove_and_package_list_agree_with_cargo() {
    if !cargo_available() {
        return;
    }
    let pair = Pair::new(|root| fs::create_dir_all(root).unwrap());
    let tree = |dir: &Path| -> Vec<(String, String)> {
        let mut out = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            for e in fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                let rel = p
                    .strip_prefix(dir)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                if rel.starts_with(".git/") || rel == ".git" || rel.starts_with("target") {
                    continue;
                }
                if p.is_dir() {
                    stack.push(p);
                } else {
                    out.push((rel, fs::read_to_string(&p).unwrap_or_default()));
                }
            }
        }
        out.sort();
        out
    };
    for args in [
        &["new", "--vcs", "none", "bin1"][..],
        &["new", "--vcs", "none", "--lib", "lib1"],
        &["new", "--vcs", "none", "--edition", "2018", "old1"],
    ] {
        let (c, m) = pair.both(".", args);
        assert!(
            c.status.success() && m.status.success(),
            "{}{}",
            err(&c),
            err(&m)
        );
        let name = args.last().unwrap();
        assert_eq!(
            tree(&pair.cargo_root.join(name)),
            tree(&pair.mitts_root.join(name)),
            "{}",
            args.join(" ")
        );
    }
    for root in [&pair.cargo_root, &pair.mitts_root] {
        fs::create_dir_all(root.join("plain")).unwrap();
    }
    let (c, m) = pair.both("plain", &["init", "--vcs", "none", "--name", "plain"]);
    assert!(
        c.status.success() && m.status.success(),
        "{}{}",
        err(&c),
        err(&m)
    );
    assert_eq!(
        tree(&pair.cargo_root.join("plain")),
        tree(&pair.mitts_root.join("plain")),
        "init"
    );

    // Manifest edits with path dependencies (offline).
    for args in [
        &["add", "--offline", "--path", "../lib1"][..],
        &[
            "add",
            "--offline",
            "--path",
            "../lib1",
            "--rename",
            "l1",
            "--dev",
        ],
        &[
            "add",
            "--offline",
            "--path",
            "../old1",
            "--optional",
            "--features",
            "",
        ],
        &["remove", "--offline", "--dev", "l1"],
    ] {
        let args: Vec<&str> = args.iter().copied().filter(|a| !a.is_empty()).collect();
        let (c, m) = pair.both("bin1", &args);
        assert_eq!(
            c.status.success(),
            m.status.success(),
            "{}: {}\n{}",
            args.join(" "),
            err(&c),
            err(&m)
        );
        assert_eq!(
            fs::read_to_string(pair.cargo_root.join("bin1/Cargo.toml")).unwrap(),
            fs::read_to_string(pair.mitts_root.join("bin1/Cargo.toml")).unwrap(),
            "{}",
            args.join(" ")
        );
    }

    let (c, m) = pair.both("lib1", &["package", "--list", "--allow-dirty", "--offline"]);
    agree("package --list", &c, &m, &pair);
}
