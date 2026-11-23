/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Cargo's commands as Mitts runs them: new, init, locate-project, pkgid, version,
//! generate-lockfile, update, tree, add, remove, `-C`, `rust-version` and the test target
//! selectors. Registry work goes to a local mock registry; every run has its own
//! CARGO_HOME and cache.

#![cfg(unix)]

#[path = "support/mock_registry.rs"]
mod mock_registry;

use mock_registry::{dep, serve, version, Published, Registry};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;

const CLEARED: [&str; 9] = [
    "CARGO_TARGET_DIR",
    "CARGO_BUILD_TARGET_DIR",
    "CARGO_BUILD_TARGET",
    "RUSTC_WRAPPER",
    "RUSTC_WORKSPACE_WRAPPER",
    "CARGO_BUILD_RUSTC_WRAPPER",
    "CARGO_MAKEFLAGS",
    "MAKEFLAGS",
    "MITTS_FAST_LOCK",
];

struct Env {
    temp: TempDir,
}

impl Env {
    fn new() -> Env {
        Env {
            temp: TempDir::new().unwrap(),
        }
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.temp.path().join(rel)
    }

    /// Runs Mitts in `dir` with an isolated home, cache and no route to crates.io.
    fn run(&self, dir: &Path, args: &[&str]) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mitts"));
        cmd.args(args)
            .current_dir(dir)
            .env("CARGO_HOME", self.path("cargo-home"))
            .env("MITTS_CACHE_DIR", self.path("cache"))
            .env("MITTS_SPARSE_INDEX_URL", "http://127.0.0.1:9")
            .env("MITTS_CRATES_IO_URL", "http://127.0.0.1:9")
            .env("MITTS_NET_RETRY", "0")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.com")
            .env("CI", "true");
        for name in CLEARED {
            cmd.env_remove(name);
        }
        cmd.output().expect("run mitts")
    }

    fn ok(&self, dir: &Path, args: &[&str]) -> (String, String) {
        let out = self.run(dir, args);
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
        assert!(
            out.status.success(),
            "mitts {args:?} failed\n--- stdout\n{stdout}\n--- stderr\n{stderr}"
        );
        (stdout, stderr)
    }

    fn fails(&self, dir: &Path, args: &[&str]) -> String {
        let out = self.run(dir, args);
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(!out.status.success(), "mitts {args:?} succeeded:\n{text}");
        text
    }

    /// A package `app` using `registry` for crates.io, with `deps` under [dependencies].
    fn app(&self, registry: &Registry, manifest_tail: &str) -> PathBuf {
        let dir = self.path("app");
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::create_dir_all(dir.join(".cargo")).unwrap();
        fs::write(dir.join(".cargo/config.toml"), registry.source_replacement()).unwrap();
        fs::write(
            dir.join("Cargo.toml"),
            format!("[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n{manifest_tail}"),
        )
        .unwrap();
        fs::write(dir.join("src/main.rs"), "fn main() {}\n").unwrap();
        dir
    }
}

fn has_git() -> bool {
    Command::new("git").arg("--version").output().is_ok_and(|o| o.status.success())
}

fn read(path: impl AsRef<Path>) -> String {
    fs::read_to_string(path).unwrap()
}

/// (name, version) of every registry package in a Cargo.lock.
fn locked(dir: &Path) -> Vec<(String, String)> {
    let lock: toml::Value = toml::from_str(&read(dir.join("Cargo.lock"))).unwrap();
    let mut out: Vec<(String, String)> = lock["package"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p.get("source").is_some())
        .map(|p| {
            (
                p["name"].as_str().unwrap().to_string(),
                p["version"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    out.sort();
    out
}

fn pair(name: &str, version: &str) -> (String, String) {
    (name.to_string(), version.to_string())
}

#[test]
fn new_writes_cargos_files_and_checks_the_name() {
    let env = Env::new();
    let base = env.path("work");
    fs::create_dir_all(&base).unwrap();

    let (_, err) = env.ok(&base, &["new", "hello"]);
    assert!(err.contains("Creating binary (application) `hello` package"), "{err}");
    let manifest = read(base.join("hello/Cargo.toml"));
    assert!(manifest.starts_with("[package]\nname = \"hello\"\nversion = \"0.1.0\"\nedition = \""), "{manifest}");
    assert!(manifest.ends_with("\n[dependencies]\n"), "{manifest}");
    assert_eq!(
        read(base.join("hello/src/main.rs")),
        "fn main() {\n    println!(\"Hello, world!\");\n}\n"
    );
    if has_git() {
        assert!(base.join("hello/.git").is_dir());
        assert_eq!(read(base.join("hello/.gitignore")), "/target\n");
    }

    env.ok(&base, &["new", "--lib", "mylib", "--edition", "2021", "--vcs", "none"]);
    assert!(read(base.join("mylib/Cargo.toml")).contains("edition = \"2021\""));
    assert!(read(base.join("mylib/src/lib.rs")).contains("pub fn add(left: u64, right: u64) -> u64"));
    assert!(!base.join("mylib/.gitignore").exists());
    assert!(!base.join("mylib/.git").exists());

    let err = env.fails(&base, &["new", "2fast"]);
    assert!(err.contains("the name cannot start with a digit"), "{err}");
    let err = env.fails(&base, &["new", "hello"]);
    assert!(err.contains("destination") && err.contains("already exists"), "{err}");
    let err = env.fails(&base, &["new", "fn", "--vcs", "none"]);
    assert!(err.contains("it is a Rust keyword"), "{err}");

    // The built package runs.
    let (out, _) = env.ok(&base.join("hello"), &["run", "-q"]);
    assert_eq!(out, "Hello, world!\n");
}

#[test]
fn init_adopts_existing_sources_and_refuses_a_package() {
    let env = Env::new();
    let dir = env.path("existing");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("src/lib.rs"), "pub fn kept() {}\n").unwrap();
    let (_, err) = env.ok(&dir, &["init", "--vcs", "none"]);
    assert!(err.contains("Creating library package"), "{err}");
    assert_eq!(read(dir.join("src/lib.rs")), "pub fn kept() {}\n");
    assert!(read(dir.join("Cargo.toml")).contains("name = \"existing\""));
    assert!(!dir.join("src/main.rs").exists());
    let err = env.fails(&dir, &["init"]);
    assert!(err.contains("cannot be run on existing Cargo packages"), "{err}");

    let root_main = env.path("loose");
    fs::create_dir_all(&root_main).unwrap();
    fs::write(root_main.join("main.rs"), "fn main() {}\n").unwrap();
    env.ok(&root_main, &["init", "--vcs", "none", "--name", "loose_bin"]);
    let manifest = read(root_main.join("Cargo.toml"));
    assert!(manifest.contains("[[bin]]\nname = \"loose_bin\"\npath = \"main.rs\""), "{manifest}");
}

#[test]
fn new_inside_a_workspace_joins_its_members() {
    let env = Env::new();
    let ws = env.path("ws");
    fs::create_dir_all(ws.join("crates")).unwrap();
    fs::write(
        ws.join("Cargo.toml"),
        "# the workspace\n[workspace]\nresolver = \"2\"\nmembers = [\"crates/a\"]\n",
    )
    .unwrap();
    env.ok(&ws, &["new", "--vcs", "none", "crates/a"]);
    let (_, err) = env.ok(&ws, &["new", "--vcs", "none", "--lib", "crates/b"]);
    assert!(err.contains("Adding `b` as member of workspace"), "{err}");
    let text = read(ws.join("Cargo.toml"));
    assert!(text.starts_with("# the workspace\n"), "{text}");
    assert!(text.contains("\"crates/a\"") && text.contains("\"crates/b\""), "{text}");
    assert_eq!(text.matches("crates/a").count(), 1, "{text}");

    fs::write(
        ws.join("Cargo.toml"),
        "[workspace]\nresolver = \"2\"\nmembers = [\"crates/*\"]\n",
    )
    .unwrap();
    env.ok(&ws, &["new", "--vcs", "none", "crates/c"]);
    assert_eq!(read(ws.join("Cargo.toml")), "[workspace]\nresolver = \"2\"\nmembers = [\"crates/*\"]\n");
    // The workspace builds with its new members.
    env.ok(&ws, &["check", "--workspace", "-q"]);
}

#[test]
fn locate_project_pkgid_version_and_change_directory() {
    let env = Env::new();
    let registry = serve(vec![version("aa", "1.0.0")]);
    let app = env.app(&registry, "[dependencies]\naa = \"1\"\n");
    let canonical = app.canonicalize().unwrap();
    let nested = app.join("src");

    let (out, _) = env.ok(&nested, &["locate-project"]);
    let value: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(
        Path::new(value["root"].as_str().unwrap()).canonicalize().unwrap(),
        canonical.join("Cargo.toml")
    );
    let (out, _) = env.ok(&nested, &["locate-project", "--message-format", "plain", "--workspace"]);
    assert_eq!(Path::new(out.trim()).canonicalize().unwrap(), canonical.join("Cargo.toml"));

    let parent = app.parent().unwrap().to_path_buf();
    let (out, _) = env.ok(&parent, &["-C", "app", "locate-project", "--message-format", "plain"]);
    assert_eq!(Path::new(out.trim()).canonicalize().unwrap(), canonical.join("Cargo.toml"));

    let (out, _) = env.ok(&app, &["pkgid"]);
    assert!(out.trim().starts_with("path+file://") && out.trim().ends_with("/app#0.1.0"), "{out}");
    let (out, _) = env.ok(&app, &["pkgid", "aa"]);
    assert_eq!(out.trim(), "registry+https://github.com/rust-lang/crates.io-index#aa@1.0.0");
    let err = env.fails(&app, &["pkgid", "nope"]);
    assert!(err.contains("did not match any packages"), "{err}");

    let (out, _) = env.ok(&app, &["version"]);
    assert!(out.starts_with(&format!("mitts {}", env!("CARGO_PKG_VERSION"))), "{out}");
    let (out, _) = env.ok(&app, &["version", "-v"]);
    assert!(out.contains("\nrelease: ") && out.contains("\ncommit-hash: "), "{out}");
}

fn two_crates() -> Vec<Published> {
    vec![
        version("aa", "1.0.0"),
        version("aa", "1.0.1"),
        version("bb", "1.0.0"),
        version("bb", "1.0.1"),
    ]
}

const OLD_LOCK: &str = "version = 4\n\n[[package]]\nname = \"aa\"\nversion = \"1.0.0\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\n\n[[package]]\nname = \"app\"\nversion = \"0.1.0\"\ndependencies = [\"aa\", \"bb\"]\n\n[[package]]\nname = \"bb\"\nversion = \"1.0.0\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\n";

#[test]
fn update_moves_only_what_is_asked() {
    let env = Env::new();
    let registry = serve(two_crates());
    let app = env.app(&registry, "[dependencies]\naa = \"1\"\nbb = \"1\"\n");
    fs::write(app.join("Cargo.lock"), OLD_LOCK).unwrap();

    let (_, err) = env.ok(&app, &["update", "aa", "--dry-run"]);
    assert!(err.contains("Updating aa v1.0.0 -> v1.0.1"), "{err}");
    assert!(err.contains("warning: aborting update due to dry run"), "{err}");
    assert_eq!(read(app.join("Cargo.lock")), OLD_LOCK);

    let err = env.fails(&app, &["update", "aa", "--locked"]);
    assert!(err.contains("--locked was passed"), "{err}");

    let (_, err) = env.ok(&app, &["update", "-p", "aa"]);
    assert!(err.contains("Updating aa v1.0.0 -> v1.0.1"), "{err}");
    assert_eq!(locked(&app), [pair("aa", "1.0.1"), pair("bb", "1.0.0")]);
    assert!(read(app.join("Cargo.lock")).contains("\nversion = 4\n"));

    env.ok(&app, &["update", "aa", "--precise", "1.0.0"]);
    assert_eq!(locked(&app), [pair("aa", "1.0.0"), pair("bb", "1.0.0")]);
    let err = env.fails(&app, &["update", "aa", "--precise", "9.9.9"]);
    assert!(err.contains("failed to select a version"), "{err}");

    env.ok(&app, &["update", "--workspace"]);
    assert_eq!(locked(&app), [pair("aa", "1.0.0"), pair("bb", "1.0.0")]);

    let (_, err) = env.ok(&app, &["update"]);
    assert!(err.contains("Locking 2 packages to latest compatible versions"), "{err}");
    assert_eq!(locked(&app), [pair("aa", "1.0.1"), pair("bb", "1.0.1")]);

    fs::write(app.join("Cargo.lock"), OLD_LOCK).unwrap();
    env.ok(&app, &["generate-lockfile"]);
    assert_eq!(locked(&app), [pair("aa", "1.0.1"), pair("bb", "1.0.1")]);
    let err = env.fails(&app, &["generate-lockfile", "--locked"]);
    assert!(err.contains("needs to be updated"), "{err}");
}

#[test]
fn tree_prints_like_cargo() {
    let env = Env::new();
    let mut aa = version("aa", "1.0.0");
    aa.deps = vec![dep("bb", "1")];
    let registry = serve(vec![
        aa,
        version("bb", "1.0.0"),
        version("cc", "1.0.0"),
        version("dd", "1.0.0"),
    ]);
    let app = env.app(
        &registry,
        "[dependencies]\naa = \"1\"\nbb = \"1\"\n\n[build-dependencies]\ncc = \"1\"\n\n[dev-dependencies]\ndd = \"1\"\n",
    );
    fs::write(app.join("build.rs"), "fn main() {}\n").unwrap();
    let canonical = app.canonicalize().unwrap();
    let (out, _) = env.ok(&app, &["tree"]);
    let expected = format!(
        "app v0.1.0 ({})\n├── aa v1.0.0\n│   └── bb v1.0.0\n└── bb v1.0.0\n[build-dependencies]\n└── cc v1.0.0\n[dev-dependencies]\n└── dd v1.0.0\n",
        canonical.display()
    );
    let shown = out.replace(&app.display().to_string(), &canonical.display().to_string());
    assert_eq!(shown, expected);

    let (out, _) = env.ok(&app, &["tree", "-e", "normal", "--prefix", "depth", "--charset", "ascii"]);
    assert!(out.contains("\n1aa v1.0.0\n2bb v1.0.0\n1bb v1.0.0\n"), "{out}");
    assert!(!out.contains("cc v1.0.0") && !out.contains("dd v1.0.0"), "{out}");

    let (out, _) = env.ok(&app, &["tree", "-i", "bb", "-e", "normal"]);
    assert!(out.starts_with("bb v1.0.0\n├── aa v1.0.0\n│   └── app v0.1.0"), "{out}");
    let (out, _) = env.ok(&app, &["tree", "--depth", "1", "-e", "normal"]);
    assert!(!out.contains("│   └── bb"), "{out}");
    let (out, _) = env.ok(&app, &["tree", "-e", "normal", "-f", "{p} [{l}]"]);
    assert!(out.contains("aa v1.0.0 []"), "{out}");
    let err = env.fails(&app, &["tree", "-e", "bogus"]);
    assert!(err.contains("unknown edge kind"), "{err}");
}

#[test]
fn add_and_remove_edit_the_manifest_and_lock() {
    let env = Env::new();
    let mut aa = version("aa", "1.2.0");
    aa.features = "{\"default\":[\"std\"],\"std\":[],\"extra\":[]}";
    let registry = serve(vec![
        version("aa", "1.0.0"),
        aa,
        version("bb", "0.3.0"),
        Published {
            features: "{\"x\":[]}",
            ..version("cc", "2.0.0")
        },
        Published {
            yanked: true,
            ..version("aa", "1.3.0")
        },
    ]);
    let app = env.app(&registry, "# keep this comment\n[dependencies]\n");
    let local = env.path("local");
    fs::create_dir_all(local.join("src")).unwrap();
    fs::write(local.join("Cargo.toml"), "[package]\nname = \"local\"\nversion = \"0.2.0\"\nedition = \"2021\"\n").unwrap();
    fs::write(local.join("src/lib.rs"), "").unwrap();

    let (_, err) = env.ok(&app, &["add", "aa"]);
    assert!(err.contains("Adding aa v1.2.0 to dependencies"), "{err}");
    assert!(err.contains("+ std") && err.contains("- extra"), "{err}");
    let (_, err) = env.ok(&app, &["add", "bb@0.3", "--dev"]);
    assert!(err.contains("Adding bb v0.3.0 to dev-dependencies"), "{err}");
    env.ok(&app, &["add", "cc", "--build", "--no-default-features", "-F", "x"]);
    env.ok(&app, &["add", "cc", "--rename", "cc2"]);
    env.ok(&app, &["add", "--path", "../local"]);
    let text = read(app.join("Cargo.toml"));
    assert!(text.contains("# keep this comment\n[dependencies]\naa = \"1.2.0\"\n"), "{text}");
    assert!(text.contains("cc2 = { version = \"2.0.0\", package = \"cc\" }"), "{text}");
    assert!(text.contains("local = { path = \"../local\" }"), "{text}");
    assert!(text.contains("[dev-dependencies]\nbb = \"0.3\"\n"), "{text}");
    assert!(
        text.contains("[build-dependencies]\ncc = { version = \"2.0.0\", default-features = false, features = [\"x\"] }"),
        "{text}"
    );
    assert_eq!(locked(&app), [pair("aa", "1.2.0"), pair("bb", "0.3.0"), pair("cc", "2.0.0")]);

    let err = env.fails(&app, &["add", "nosuch"]);
    assert!(err.contains("could not find `nosuch`"), "{err}");
    let err = env.fails(&app, &["add", "bb", "-F", "nope"]);
    assert!(err.contains("unrecognized feature for crate bb: nope"), "{err}");
    env.ok(&app, &["add", "aa", "--dry-run", "-F", "extra"]);
    assert!(read(app.join("Cargo.toml")).contains("aa = \"1.2.0\"\n"));

    let (_, err) = env.ok(&app, &["remove", "aa"]);
    assert!(err.contains("Removing aa from dependencies"), "{err}");
    env.ok(&app, &["rm", "bb", "--dev"]);
    let text = read(app.join("Cargo.toml"));
    assert!(!text.contains("aa =") && !text.contains("bb ="), "{text}");
    assert_eq!(locked(&app), [pair("cc", "2.0.0")]);
    let err = env.fails(&app, &["remove", "zz"]);
    assert!(err.contains("the dependency `zz` could not be found in `dependencies`"), "{err}");
    env.ok(&app, &["check", "-q"]);
}

#[test]
fn rust_version_is_enforced_unless_ignored() {
    let env = Env::new();
    let registry = serve(Vec::new());
    let app = env.app(&registry, "");
    let manifest = read(app.join("Cargo.toml")).replace(
        "edition = \"2021\"\n",
        "edition = \"2021\"\nrust-version = \"1.999\"\n",
    );
    fs::write(app.join("Cargo.toml"), manifest).unwrap();
    let err = env.fails(&app, &["check"]);
    assert!(err.contains("is not supported by the following package"), "{err}");
    assert!(err.contains("app@0.1.0 requires rustc 1.999"), "{err}");
    env.ok(&app, &["check", "--ignore-rust-version", "-q"]);
}

#[test]
fn test_selects_examples_and_benches_by_name() {
    let env = Env::new();
    let registry = serve(Vec::new());
    let app = env.app(&registry, "");
    fs::create_dir_all(app.join("examples")).unwrap();
    fs::create_dir_all(app.join("benches")).unwrap();
    fs::write(
        app.join("examples/demo.rs"),
        "fn main() {}\n#[test]\nfn demo_example_test() {}\n",
    )
    .unwrap();
    fs::write(
        app.join("benches/speed.rs"),
        "fn main() {}\n#[test]\nfn speed_bench_test() {}\n",
    )
    .unwrap();
    let (out, err) = env.ok(&app, &["test", "--example", "demo"]);
    let all = format!("{out}{err}");
    assert!(all.contains("demo_example_test"), "{all}");
    assert!(!all.contains("speed_bench_test"), "{all}");
    let (out, err) = env.ok(&app, &["test", "--examples"]);
    assert!(format!("{out}{err}").contains("demo_example_test"));
    let (out, err) = env.ok(&app, &["test", "--bench", "speed"]);
    let all = format!("{out}{err}");
    assert!(all.contains("speed_bench_test") && !all.contains("demo_example_test"), "{all}");
}
