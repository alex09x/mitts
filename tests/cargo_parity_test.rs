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
        fs::write(
            dir.join(".cargo/config.toml"),
            registry.source_replacement(),
        )
        .unwrap();
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
    Command::new("git")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
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
    assert!(
        err.contains("Creating binary (application) `hello` package"),
        "{err}"
    );
    let manifest = read(base.join("hello/Cargo.toml"));
    assert!(
        manifest.starts_with("[package]\nname = \"hello\"\nversion = \"0.1.0\"\nedition = \""),
        "{manifest}"
    );
    assert!(manifest.ends_with("\n[dependencies]\n"), "{manifest}");
    assert_eq!(
        read(base.join("hello/src/main.rs")),
        "fn main() {\n    println!(\"Hello, world!\");\n}\n"
    );
    if has_git() {
        assert!(base.join("hello/.git").is_dir());
        assert_eq!(read(base.join("hello/.gitignore")), "/target\n");
    }

    env.ok(
        &base,
        &[
            "new",
            "--lib",
            "mylib",
            "--edition",
            "2021",
            "--vcs",
            "none",
        ],
    );
    assert!(read(base.join("mylib/Cargo.toml")).contains("edition = \"2021\""));
    assert!(
        read(base.join("mylib/src/lib.rs")).contains("pub fn add(left: u64, right: u64) -> u64")
    );
    assert!(!base.join("mylib/.gitignore").exists());
    assert!(!base.join("mylib/.git").exists());

    let err = env.fails(&base, &["new", "2fast"]);
    assert!(err.contains("the name cannot start with a digit"), "{err}");
    let err = env.fails(&base, &["new", "hello"]);
    assert!(
        err.contains("destination") && err.contains("already exists"),
        "{err}"
    );
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
    assert!(
        err.contains("cannot be run on existing Cargo packages"),
        "{err}"
    );

    let root_main = env.path("loose");
    fs::create_dir_all(&root_main).unwrap();
    fs::write(root_main.join("main.rs"), "fn main() {}\n").unwrap();
    env.ok(
        &root_main,
        &["init", "--vcs", "none", "--name", "loose_bin"],
    );
    let manifest = read(root_main.join("Cargo.toml"));
    assert!(
        manifest.contains("[[bin]]\nname = \"loose_bin\"\npath = \"main.rs\""),
        "{manifest}"
    );
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
    assert!(
        text.contains("\"crates/a\"") && text.contains("\"crates/b\""),
        "{text}"
    );
    assert_eq!(text.matches("crates/a").count(), 1, "{text}");

    fs::write(
        ws.join("Cargo.toml"),
        "[workspace]\nresolver = \"2\"\nmembers = [\"crates/*\"]\n",
    )
    .unwrap();
    env.ok(&ws, &["new", "--vcs", "none", "crates/c"]);
    assert_eq!(
        read(ws.join("Cargo.toml")),
        "[workspace]\nresolver = \"2\"\nmembers = [\"crates/*\"]\n"
    );
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
        Path::new(value["root"].as_str().unwrap())
            .canonicalize()
            .unwrap(),
        canonical.join("Cargo.toml")
    );
    let (out, _) = env.ok(
        &nested,
        &["locate-project", "--message-format", "plain", "--workspace"],
    );
    assert_eq!(
        Path::new(out.trim()).canonicalize().unwrap(),
        canonical.join("Cargo.toml")
    );

    let parent = app.parent().unwrap().to_path_buf();
    let (out, _) = env.ok(
        &parent,
        &["-C", "app", "locate-project", "--message-format", "plain"],
    );
    assert_eq!(
        Path::new(out.trim()).canonicalize().unwrap(),
        canonical.join("Cargo.toml")
    );

    let (out, _) = env.ok(&app, &["pkgid"]);
    assert!(
        out.trim().starts_with("path+file://") && out.trim().ends_with("/app#0.1.0"),
        "{out}"
    );
    let (out, _) = env.ok(&app, &["pkgid", "aa"]);
    assert_eq!(
        out.trim(),
        "registry+https://github.com/rust-lang/crates.io-index#aa@1.0.0"
    );
    let err = env.fails(&app, &["pkgid", "nope"]);
    assert!(err.contains("did not match any packages"), "{err}");

    let (out, _) = env.ok(&app, &["version"]);
    assert!(
        out.starts_with(&format!("mitts {}", env!("CARGO_PKG_VERSION"))),
        "{out}"
    );
    let (out, _) = env.ok(&app, &["version", "-v"]);
    assert!(
        out.contains("\nrelease: ") && out.contains("\ncommit-hash: "),
        "{out}"
    );
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
    assert!(
        err.contains("warning: aborting update due to dry run"),
        "{err}"
    );
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
    assert!(
        err.contains("Locking 2 packages to latest compatible versions"),
        "{err}"
    );
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
    // macOS: the temp dir is /var/…, its canonical form /private/var/….
    let canonical_text = canonical.display().to_string();
    let shown = if out.contains(&canonical_text) {
        out.clone()
    } else {
        out.replace(&app.display().to_string(), &canonical_text)
    };
    assert_eq!(shown, expected);

    let (out, _) = env.ok(
        &app,
        &[
            "tree",
            "-e",
            "normal",
            "--prefix",
            "depth",
            "--charset",
            "ascii",
        ],
    );
    assert!(
        out.contains("\n1aa v1.0.0\n2bb v1.0.0\n1bb v1.0.0\n"),
        "{out}"
    );
    assert!(
        !out.contains("cc v1.0.0") && !out.contains("dd v1.0.0"),
        "{out}"
    );

    let (out, _) = env.ok(&app, &["tree", "-i", "bb", "-e", "normal"]);
    assert!(
        out.starts_with("bb v1.0.0\n├── aa v1.0.0\n│   └── app v0.1.0"),
        "{out}"
    );
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
    fs::write(
        local.join("Cargo.toml"),
        "[package]\nname = \"local\"\nversion = \"0.2.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(local.join("src/lib.rs"), "").unwrap();

    let (_, err) = env.ok(&app, &["add", "aa"]);
    assert!(err.contains("Adding aa v1.2.0 to dependencies"), "{err}");
    assert!(err.contains("+ std") && err.contains("- extra"), "{err}");
    let (_, err) = env.ok(&app, &["add", "bb@0.3", "--dev"]);
    assert!(
        err.contains("Adding bb v0.3.0 to dev-dependencies"),
        "{err}"
    );
    env.ok(
        &app,
        &["add", "cc", "--build", "--no-default-features", "-F", "x"],
    );
    env.ok(&app, &["add", "cc", "--rename", "cc2"]);
    env.ok(&app, &["add", "--path", "../local"]);
    let text = read(app.join("Cargo.toml"));
    assert!(
        text.contains("# keep this comment\n[dependencies]\naa = \"1.2.0\"\n"),
        "{text}"
    );
    assert!(
        text.contains("cc2 = { version = \"2.0.0\", package = \"cc\" }"),
        "{text}"
    );
    // As Cargo: the package's version beside its path.
    assert!(
        text.contains("local = { version = \"0.2.0\", path = \"../local\" }"),
        "{text}"
    );
    assert!(
        text.contains("[dev-dependencies]\nbb = \"0.3\"\n"),
        "{text}"
    );
    assert!(
        text.contains("[build-dependencies]\ncc = { version = \"2.0.0\", default-features = false, features = [\"x\"] }"),
        "{text}"
    );
    assert_eq!(
        locked(&app),
        [
            pair("aa", "1.2.0"),
            pair("bb", "0.3.0"),
            pair("cc", "2.0.0")
        ]
    );

    let err = env.fails(&app, &["add", "nosuch"]);
    assert!(err.contains("could not find `nosuch`"), "{err}");
    let err = env.fails(&app, &["add", "bb", "-F", "nope"]);
    assert!(
        err.contains("unrecognized feature for crate bb: nope"),
        "{err}"
    );
    env.ok(&app, &["add", "aa", "--dry-run", "-F", "extra"]);
    assert!(read(app.join("Cargo.toml")).contains("aa = \"1.2.0\"\n"));

    let (_, err) = env.ok(&app, &["remove", "aa"]);
    assert!(err.contains("Removing aa from dependencies"), "{err}");
    env.ok(&app, &["rm", "bb", "--dev"]);
    let text = read(app.join("Cargo.toml"));
    assert!(!text.contains("aa =") && !text.contains("bb ="), "{text}");
    assert_eq!(locked(&app), [pair("cc", "2.0.0")]);
    let err = env.fails(&app, &["remove", "zz"]);
    assert!(
        err.contains("the dependency `zz` could not be found in `dependencies`"),
        "{err}"
    );
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
    assert!(
        err.contains("is not supported by the following package"),
        "{err}"
    );
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
    assert!(
        all.contains("speed_bench_test") && !all.contains("demo_example_test"),
        "{all}"
    );
}

/// `[registries.mock]` for the API commands, beside the crates.io replacement.
fn with_named_registry(app: &Path, registry: &Registry) {
    let config = format!(
        "{}\n[registries.mock]\nindex = \"{}\"\n",
        registry.source_replacement(),
        registry.index_url()
    );
    fs::write(app.join(".cargo/config.toml"), config).unwrap();
}

#[test]
fn doc_documents_the_package_and_its_dependencies_once() {
    let env = Env::new();
    let registry = serve(vec![Published {
        files: vec![(
            "src/lib.rs",
            "/// Says hello.\npub fn hello() {}\n".to_string(),
        )],
        ..version("aa", "1.0.0")
    }]);
    let app = env.app(&registry, "[dependencies]\naa = \"1\"\n");
    fs::write(
        app.join("src/lib.rs"),
        "//! The app.\npub fn app() { aa::hello() }\n",
    )
    .unwrap();

    let (_, err) = env.ok(&app, &["doc"]);
    assert!(err.contains("Documenting aa v1.0.0"), "{err}");
    assert!(err.contains("Documenting app v0.1.0"), "{err}");
    assert!(err.contains("Generated"), "{err}");
    assert!(app.join("target/doc/app/index.html").is_file());
    assert!(app.join("target/doc/aa/index.html").is_file());
    assert!(read(app.join("target/doc/aa/fn.hello.html")).contains("Says hello."));

    let (_, err) = env.ok(&app, &["doc"]);
    assert!(
        !err.contains("Documenting"),
        "nothing changed, nothing documented again: {err}"
    );

    fs::remove_dir_all(app.join("target/doc")).unwrap();
    env.ok(&app, &["doc", "--no-deps"]);
    assert!(app.join("target/doc/app/index.html").is_file());
    assert!(!app.join("target/doc/aa").exists());

    fs::write(
        app.join("src/lib.rs"),
        "//! The app.\nfn private_helper() {}\npub fn app() { private_helper() }\n",
    )
    .unwrap();
    env.ok(&app, &["doc", "--no-deps", "--document-private-items"]);
    assert!(app.join("target/doc/app/fn.private_helper.html").is_file());
}

#[test]
fn fix_applies_machine_applicable_suggestions_and_guards_uncommitted_work() {
    let env = Env::new();
    let registry = serve(Vec::new());
    let app = env.app(&registry, "");
    fs::write(
        app.join("src/main.rs"),
        "fn main() {\n    let mut x = 1;\n    println!(\"{x}\");\n}\n",
    )
    .unwrap();

    let err = env.fails(&app, &["fix"]);
    assert!(err.contains("no VCS found"), "{err}");
    let (_, err) = env.ok(&app, &["fix", "--allow-no-vcs"]);
    assert!(err.contains("Fixed src/main.rs (1 fix)"), "{err}");
    assert_eq!(
        read(app.join("src/main.rs")),
        "fn main() {\n    let x = 1;\n    println!(\"{x}\");\n}\n"
    );

    if has_git() {
        let git = |args: &[&str]| {
            let ok = Command::new("git")
                .args(args)
                .current_dir(&app)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@example.com")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@example.com")
                .status()
                .unwrap()
                .success();
            assert!(ok, "git {args:?}");
        };
        git(&["init", "-q"]);
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "init"]);
        fs::write(
            app.join("src/main.rs"),
            "fn main() {\n    let mut y = 2;\n    println!(\"{y}\");\n}\n",
        )
        .unwrap();
        let err = env.fails(&app, &["fix"]);
        assert!(
            err.contains("uncommitted changes") && err.contains("src/main.rs (dirty)"),
            "{err}"
        );
        env.ok(&app, &["fix", "--allow-dirty"]);
        assert!(read(app.join("src/main.rs")).contains("let y = 2;"));
    }
}

#[test]
fn vendor_copies_sources_with_checksums_and_the_copy_builds() {
    let env = Env::new();
    let mut aa = version("aa", "1.0.0");
    aa.deps = vec![dep("bb", "1")];
    let registry = serve(vec![aa, version("bb", "1.0.0")]);
    let app = env.app(&registry, "[dependencies]\naa = \"1\"\n");
    fs::write(
        app.join("src/main.rs"),
        "fn main() { println!(\"{}\", aa::hello()); }\n",
    )
    .unwrap();

    let (out, err) = env.ok(&app, &["vendor"]);
    assert!(err.contains("Vendoring aa v1.0.0"), "{err}");
    assert!(
        out.contains("[source.crates-io]\nreplace-with = \"vendored-sources\""),
        "{out}"
    );
    assert!(
        out.contains("[source.vendored-sources]\ndirectory = \"vendor\""),
        "{out}"
    );
    let checksum: serde_json::Value =
        serde_json::from_str(&read(app.join("vendor/aa/.cargo-checksum.json"))).unwrap();
    assert_eq!(checksum["package"].as_str().unwrap().len(), 64);
    assert!(checksum["files"]["Cargo.toml"].is_string());
    assert!(app.join("vendor/bb/src/lib.rs").is_file());

    // The printed configuration is all the project needs to build offline.
    fs::write(app.join(".cargo/config.toml"), out).unwrap();
    let (stdout, _) = env.ok(&app, &["run", "-q", "--offline"]);
    assert_eq!(stdout, "aa\n");

    // A stale copy goes, unless --no-delete.
    fs::create_dir_all(app.join("vendor/old")).unwrap();
    fs::write(app.join("vendor/old/.cargo-checksum.json"), "{}").unwrap();
    env.ok(&app, &["vendor", "--offline", "--no-delete"]);
    assert!(app.join("vendor/old").exists());
    env.ok(&app, &["vendor", "--offline"]);
    assert!(!app.join("vendor/old").exists());
}

#[test]
fn registry_api_commands_send_what_cargo_sends() {
    let env = Env::new();
    let registry = serve(vec![
        Published {
            features: "{\"default\":[\"std\"],\"std\":[],\"extra\":[]}",
            manifest_extra: "description = \"The aa crate, for tests\"\nlicense = \"MIT\"\n",
            ..version("aa", "1.0.0")
        },
        version("aab", "0.2.0"),
    ]);
    let app = env.app(&registry, "");
    with_named_registry(&app, &registry);

    let (out, _) = env.ok(&app, &["search", "aa", "--registry", "mock"]);
    assert!(
        out.contains("aa = \"1.0.0\"") && out.contains("# The aa crate"),
        "{out}"
    );
    assert!(out.contains("aab = \"0.2.0\""), "{out}");

    let (out, _) = env.ok(&app, &["info", "aa", "--registry", "mock"]);
    assert!(
        out.starts_with("aa\nThe aa crate, for tests\nversion: 1.0.0\nlicense: MIT\n"),
        "{out}"
    );
    assert!(
        out.contains(" +default = [std]")
            && out.contains(" +std     = []")
            && out.contains("  extra   = []"),
        "{out}"
    );

    let err = env.fails(&app, &["yank", "aa@1.0.0", "--registry", "mock"]);
    assert!(err.contains("no token found"), "{err}");

    let (_, err) = env.ok(&app, &["login", "--registry", "mock", "secret-token"]);
    assert!(err.contains("Login token for `mock` saved"), "{err}");
    let creds = read(env.path("cargo-home/credentials.toml"));
    assert!(
        creds.contains("[registries.mock]") && creds.contains("token = \"secret-token\""),
        "{creds}"
    );

    let (out, _) = env.ok(&app, &["owner", "--list", "aa", "--registry", "mock"]);
    assert_eq!(out, "alice (Alice)\n");
    env.ok(&app, &["owner", "--add", "bob", "aa", "--registry", "mock"]);
    env.ok(&app, &["yank", "aa@1.0.0", "--registry", "mock"]);
    env.ok(
        &app,
        &[
            "yank",
            "aa",
            "--version",
            "1.0.0",
            "--undo",
            "--registry",
            "mock",
        ],
    );
    let sent = registry.requests();
    let put_owner = sent
        .iter()
        .find(|r| r.method == "PUT" && r.path == "/api/v1/crates/aa/owners")
        .expect("owner add request");
    assert_eq!(
        put_owner.headers.get("authorization").map(String::as_str),
        Some("secret-token")
    );
    let body: serde_json::Value = serde_json::from_slice(&put_owner.body).unwrap();
    assert_eq!(body["users"], serde_json::json!(["bob"]));
    assert!(sent.iter().any(|r| r.method == "DELETE"
        && r.path == "/api/v1/crates/aa/1.0.0/yank"
        && r.headers.get("authorization").map(String::as_str) == Some("secret-token")));
    assert!(sent
        .iter()
        .any(|r| r.method == "PUT" && r.path == "/api/v1/crates/aa/1.0.0/unyank"));

    let (_, err) = env.ok(&app, &["logout", "--registry", "mock"]);
    assert!(err.contains("has been removed from local storage"), "{err}");
    assert!(!read(env.path("cargo-home/credentials.toml")).contains("secret-token"));
}

#[test]
fn install_builds_records_and_uninstalls_binaries() {
    let env = Env::new();
    let registry = serve(vec![Published {
        files: vec![(
            "src/main.rs",
            "fn main() { println!(\"tool works\"); }\n".to_string(),
        )],
        ..version("tool", "1.0.0")
    }]);
    let app = env.app(&registry, "");
    with_named_registry(&app, &registry);
    let root = env.path("install-root");
    let root_s = root.to_str().unwrap();

    let (_, err) = env.ok(
        &app,
        &["install", "tool", "--registry", "mock", "--root", root_s],
    );
    assert!(
        err.contains("Installed package `tool v1.0.0` (executable `tool`)"),
        "{err}"
    );
    let out = Command::new(root.join("bin/tool")).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), "tool works\n");
    assert!(read(root.join(".crates.toml")).contains("tool 1.0.0 (sparse+"));
    let v2: serde_json::Value = serde_json::from_str(&read(root.join(".crates2.json"))).unwrap();
    assert_eq!(v2["installs"].as_object().unwrap().len(), 1);

    let (out, _) = env.ok(&app, &["install", "--list", "--root", root_s]);
    assert_eq!(out, "tool v1.0.0:\n    tool\n");
    let (_, err) = env.ok(
        &app,
        &["install", "tool", "--registry", "mock", "--root", root_s],
    );
    assert!(
        err.contains("is already installed, use --force to override"),
        "{err}"
    );

    // A local package by path, then both uninstalled.
    fs::write(
        app.join("src/main.rs"),
        "fn main() { println!(\"app works\"); }\n",
    )
    .unwrap();
    let (_, err) = env.ok(&app, &["install", "--path", ".", "--root", root_s]);
    assert!(err.contains("Installed package `app v0.1.0`"), "{err}");
    assert!(root.join("bin/app").is_file());
    let (_, err) = env.ok(&app, &["uninstall", "tool", "--root", root_s]);
    assert!(err.contains("Removing"), "{err}");
    assert!(!root.join("bin/tool").exists());
    env.ok(&app, &["uninstall", "app", "--root", root_s]);
    let (out, _) = env.ok(&app, &["install", "--list", "--root", root_s]);
    assert_eq!(out, "");
    let err = env.fails(&app, &["uninstall", "tool", "--root", root_s]);
    assert!(err.contains("did not match any packages"), "{err}");
}

#[test]
fn clean_narrows_by_package_doc_and_dry_run() {
    let env = Env::new();
    let registry = serve(vec![version("aa", "1.0.0")]);
    let app = env.app(&registry, "[dependencies]\naa = \"1\"\n");
    fs::write(app.join("src/lib.rs"), "pub fn app() {}\n").unwrap();
    env.ok(&app, &["build", "-q"]);
    env.ok(&app, &["doc", "-q", "--no-deps"]);
    let deps = || -> Vec<String> {
        fs::read_dir(app.join("target/debug/deps"))
            .map(|d| {
                d.flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default()
    };
    assert!(
        deps().iter().any(|f| f.starts_with("libaa-")),
        "{:?}",
        deps()
    );

    let (_, err) = env.ok(&app, &["clean", "-p", "aa", "--dry-run"]);
    assert!(
        err.contains("Summary") && err.contains("no files deleted due to --dry-run"),
        "{err}"
    );
    assert!(deps().iter().any(|f| f.starts_with("libaa-")));

    let (_, err) = env.ok(&app, &["clean", "-p", "aa"]);
    assert!(err.contains("Removed"), "{err}");
    assert!(
        !deps().iter().any(|f| f.starts_with("libaa-")),
        "{:?}",
        deps()
    );
    assert!(
        deps().iter().any(|f| f.starts_with("libapp-")),
        "{:?}",
        deps()
    );
    assert!(app.join("target/doc/app/index.html").is_file());

    env.ok(&app, &["clean", "--doc"]);
    assert!(!app.join("target/doc").exists());
    assert!(deps().iter().any(|f| f.starts_with("libapp-")));
    // The build is whole again afterwards.
    env.ok(&app, &["build", "-q"]);
}

#[test]
fn every_command_takes_config_and_more_cargo_flags() {
    let env = Env::new();
    let registry = serve(vec![version("aa", "1.0.0")]);
    let app = env.app(&registry, "[dependencies]\naa = \"1\"\n");
    fs::write(
        app.join("src/lib.rs"),
        "pub fn app() -> &'static str { aa::hello() }\n",
    )
    .unwrap();
    // `--config` on commands that have no use for it is accepted, as in Cargo.
    env.ok(&app, &["tree", "--config", "net.retry=1", "-e", "normal"]);
    env.ok(&app, &["locate-project", "--config=net.retry=1"]);
    env.ok(
        &app,
        &["fetch", "--target", "x86_64-unknown-linux-gnu", "-q"],
    );

    let (out, _) = env.ok(&app, &["metadata", "--no-deps", "--format-version", "1"]);
    let doc: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(doc["packages"].as_array().unwrap().len(), 1);
    assert!(doc["resolve"].is_null());

    let out_dir = env.path("artifacts");
    env.ok(
        &app,
        &[
            "build",
            "-q",
            "-Z",
            "unstable-options",
            "--artifact-dir",
            out_dir.to_str().unwrap(),
        ],
    );
    assert!(out_dir.join("app").is_file(), "binary copied");
    assert!(out_dir.join("libapp.rlib").is_file(), "library copied");

    let err = env.fails(&app, &["report", "future-incompatibilities"]);
    assert!(err.contains("no reports are currently available"), "{err}");
    let (_, err) = env.ok(&app, &["check", "-q", "--future-incompat-report"]);
    assert!(
        err.contains("0 dependencies had future-incompatible warnings"),
        "{err}"
    );

    let err = env.fails(
        &app,
        &[
            "generate-lockfile",
            "--publish-time",
            "2025-01-01T00:00:00Z",
        ],
    );
    assert!(err.contains("unstable"), "{err}");
}

#[test]
fn package_lists_guards_dirty_trees_and_publishes_to_a_named_registry() {
    let env = Env::new();
    let registry = serve(Vec::new());
    let app = env.app(&registry, "");
    with_named_registry(&app, &registry);
    let manifest = read(app.join("Cargo.toml")).replace(
        "edition = \"2021\"\n",
        "edition = \"2021\"\nlicense = \"MIT\"\ndescription = \"test app\"\npublish = [\"mock\"]\n",
    );
    fs::write(app.join("Cargo.toml"), manifest).unwrap();
    fs::write(app.join(".gitignore"), "/target\n").unwrap();

    let (out, _) = env.ok(&app, &["package", "--list", "--allow-dirty"]);
    assert!(
        out.lines().any(|l| l == "Cargo.toml") && out.lines().any(|l| l == "src/main.rs"),
        "{out}"
    );
    assert!(
        !app.join("target/package").exists(),
        "--list writes nothing"
    );

    if has_git() {
        let git = |args: &[&str]| {
            assert!(Command::new("git")
                .args(args)
                .current_dir(&app)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@example.com")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@example.com")
                .status()
                .unwrap()
                .success());
        };
        git(&["init", "-q"]);
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "init"]);
        fs::write(
            app.join("src/main.rs"),
            "fn main() { println!(\"changed\"); }\n",
        )
        .unwrap();
        let err = env.fails(&app, &["package", "--no-verify"]);
        assert!(
            err.contains("contain changes that were not yet committed")
                && err.contains("src/main.rs"),
            "{err}"
        );
        env.ok(&app, &["package", "--no-verify", "--allow-dirty"]);
        assert!(app.join("target/package/app-0.1.0.crate").is_file());
        git(&["commit", "-q", "-am", "change"]);
    }

    let err = env.fails(&app, &["publish", "--no-verify", "--allow-dirty"]);
    assert!(
        err.contains("does not allow publication to crates-io"),
        "{err}"
    );
    let (_, err) = env.ok(
        &app,
        &[
            "publish",
            "--no-verify",
            "--allow-dirty",
            "--registry",
            "mock",
            "--token",
            "pub-token",
        ],
    );
    assert!(
        err.contains("Uploaded app v0.1.0 to registry `mock`"),
        "{err}"
    );
    let upload = registry
        .requests()
        .into_iter()
        .find(|r| r.method == "PUT" && r.path == "/api/v1/crates/new")
        .expect("upload request");
    assert_eq!(
        upload.headers.get("authorization").map(String::as_str),
        Some("pub-token")
    );
}

fn git_in(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?}");
}

#[test]
fn fix_leaves_broken_code_and_untracked_files_alone() {
    let env = Env::new();
    let registry = serve(Vec::new());
    let app = env.app(&registry, "");
    // A fixable warning next to an error: nothing is applied without --broken-code.
    let broken =
        "fn main() {\n    let mut x = 1;\n    println!(\"{x}\");\n    let y: u32 = \"no\";\n}\n";
    fs::write(app.join("src/main.rs"), broken).unwrap();
    env.fails(&app, &["fix", "--allow-no-vcs"]);
    assert_eq!(read(app.join("src/main.rs")), broken);

    if has_git() {
        fs::write(app.join("src/main.rs"), "fn main() {}\n").unwrap();
        git_in(&app, &["init", "-q"]);
        git_in(&app, &["add", "."]);
        git_in(&app, &["commit", "-q", "-m", "init"]);
        // An untracked source file is uncommitted work too.
        fs::write(
            app.join("src/extra.rs"),
            "pub fn f() { let mut z = 1; let _ = z; }\n",
        )
        .unwrap();
        let err = env.fails(&app, &["fix"]);
        assert!(err.contains("src/extra.rs (dirty)"), "{err}");
    }
}

#[test]
fn graph_commands_skip_the_rust_version_check_and_no_deps_needs_no_index() {
    let env = Env::new();
    let registry = serve(vec![version("aa", "1.0.0")]);
    let app = env.app(&registry, "[dependencies]\naa = \"1\"\n");
    let manifest = read(app.join("Cargo.toml")).replace(
        "edition = \"2021\"\n",
        "edition = \"2021\"\nrust-version = \"1.999\"\n",
    );
    fs::write(app.join("Cargo.toml"), manifest).unwrap();
    env.ok(&app, &["tree", "-e", "normal"]);
    env.ok(&app, &["metadata", "--format-version", "1"]);
    env.fails(&app, &["check"]);

    // --no-deps: no lock, no network, only the manifests.
    fs::remove_file(app.join("Cargo.lock")).unwrap();
    let before = registry.requests().len();
    let (out, _) = env.ok(
        &app,
        &[
            "metadata",
            "--no-deps",
            "--offline",
            "--format-version",
            "1",
        ],
    );
    let doc: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(doc["packages"][0]["name"], "app");
    assert!(doc["resolve"].is_null());
    assert_eq!(
        registry.requests().len(),
        before,
        "no request for --no-deps"
    );
}

#[test]
fn an_ignored_rust_version_build_does_not_answer_for_a_checked_one() {
    let env = Env::new();
    let registry = serve(Vec::new());
    let app = env.app(&registry, "");
    let manifest = read(app.join("Cargo.toml")).replace(
        "edition = \"2021\"\n",
        "edition = \"2021\"\nrust-version = \"1.999\"\n",
    );
    fs::write(app.join("Cargo.toml"), manifest).unwrap();
    let run = |args: &[&str]| {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mitts"));
        cmd.args(args)
            .current_dir(&app)
            .env("CARGO_HOME", env.path("cargo-home"))
            .env("MITTS_CACHE_DIR", env.path("cache"))
            .env("MITTS_FAST_LOCK", "1");
        for name in CLEARED.iter().filter(|n| **n != "MITTS_FAST_LOCK") {
            cmd.env_remove(name);
        }
        cmd.output().unwrap()
    };
    for _ in 0..3 {
        assert!(run(&["build", "-q", "--ignore-rust-version"])
            .status
            .success());
    }
    let out = run(&["build", "-q"]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "the fast lock of an ignored build answered: {err}"
    );
    assert!(err.contains("requires rustc 1.999"), "{err}");
}

#[test]
fn tree_plans_the_selected_member_of_a_virtual_workspace() {
    let env = Env::new();
    let registry = serve(vec![version("aa", "1.0.0"), version("bb", "1.0.0")]);
    let ws = env.path("vws");
    fs::create_dir_all(ws.join(".cargo")).unwrap();
    fs::write(ws.join(".cargo/config.toml"), registry.source_replacement()).unwrap();
    fs::write(
        ws.join("Cargo.toml"),
        "[workspace]\nresolver = \"2\"\nmembers = [\"one\", \"two\"]\ndefault-members = [\"one\"]\n",
    )
    .unwrap();
    for (name, dep) in [("one", "aa"), ("two", "bb")] {
        fs::create_dir_all(ws.join(name).join("src")).unwrap();
        fs::write(
            ws.join(name).join("Cargo.toml"),
            format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n{dep} = \"1\"\n"),
        )
        .unwrap();
        fs::write(ws.join(name).join("src/lib.rs"), "").unwrap();
    }
    let (out, _) = env.ok(&ws, &["tree", "-p", "two", "-e", "normal"]);
    assert!(
        out.starts_with("two v0.1.0") && out.contains("└── bb v1.0.0"),
        "{out}"
    );
    assert!(!out.contains("one v0.1.0"), "{out}");
}

#[test]
fn add_keeps_expanded_tables_and_switches_sources_cleanly() {
    let env = Env::new();
    let mut aa = version("aa", "1.0.0");
    aa.features = "{\"x\":[]}";
    let registry = serve(vec![aa]);
    let app = env.app(
        &registry,
        "[dependencies.aa]\nversion = \"1.0\"\ndefault-features = false\n\n[features]\naa = [\"other\"]\nother = []\n",
    );
    env.ok(&app, &["add", "aa", "-F", "x"]);
    let text = read(app.join("Cargo.toml"));
    assert!(text.contains("[dependencies.aa]"), "{text}");
    assert!(
        text.contains("default-features = false") && text.contains("features = [\"x\"]"),
        "{text}"
    );

    let local = env.path("aa-local");
    fs::create_dir_all(local.join("src")).unwrap();
    fs::write(
        local.join("Cargo.toml"),
        "[package]\nname = \"aa\"\nversion = \"1.0.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(local.join("src/lib.rs"), "").unwrap();
    env.ok(&app, &["add", "aa", "--path", "../aa-local"]);
    let text = read(app.join("Cargo.toml"));
    assert!(
        text.contains("path = \"../aa-local\"") && !text.contains("version = \"1.0\""),
        "{text}"
    );

    // An explicit feature named like the dependency is not the dependency's.
    env.ok(&app, &["remove", "aa"]);
    let text = read(app.join("Cargo.toml"));
    assert!(text.contains("aa = [\"other\"]"), "{text}");
}

#[test]
fn packaging_and_publishing_guard_ignored_files_versions_and_destinations() {
    let env = Env::new();
    let registry = serve(Vec::new());
    let app = env.app(&registry, "");
    with_named_registry(&app, &registry);
    let manifest = read(app.join("Cargo.toml")).replace(
        "edition = \"2021\"\n",
        "edition = \"2021\"\nlicense = \"MIT\"\ndescription = \"test app\"\npublish = [\"mock\"]\n",
    );
    fs::write(app.join("Cargo.toml"), manifest).unwrap();
    fs::write(app.join(".gitignore"), "/target\ncredentials.json\n").unwrap();
    if has_git() {
        git_in(&app, &["init", "-q"]);
        git_in(&app, &["add", "."]);
        git_in(&app, &["commit", "-q", "-m", "init"]);
        fs::write(app.join("credentials.json"), "{\"secret\":1}").unwrap();
        fs::write(app.join("src/new.rs"), "").unwrap();
        let (out, _) = env.ok(&app, &["package", "--list", "--allow-dirty"]);
        assert!(out.lines().any(|l| l == "src/new.rs"), "{out}");
        assert!(
            !out.contains("credentials.json"),
            "an ignored file is never packaged: {out}"
        );
    }
    let err = env.fails(&app, &["package", "-p", "app@9.9.9", "--list"]);
    assert!(err.contains("did not match any packages"), "{err}");
    let err = env.fails(&app, &["publish", "--list"]);
    assert!(err.contains("--list"), "{err}");
    // An index with no registry name may not take a package restricted to `mock`, and a
    // stored token never goes to it.
    let err = env.fails(
        &app,
        &[
            "publish",
            "--no-verify",
            "--allow-dirty",
            "--index",
            "sparse+http://127.0.0.1:9/index/",
        ],
    );
    assert!(
        err.contains("does not allow publication to an unnamed index"),
        "{err}"
    );
}

#[test]
fn a_new_login_replaces_a_legacy_credentials_file() {
    let env = Env::new();
    let registry = serve(Vec::new());
    let app = env.app(&registry, "");
    with_named_registry(&app, &registry);
    let home = env.path("cargo-home");
    fs::create_dir_all(&home).unwrap();
    fs::write(
        home.join("credentials"),
        "[registries.mock]\ntoken = \"old\"\n",
    )
    .unwrap();
    env.ok(&app, &["login", "--registry", "mock", "new-token"]);
    assert!(!read(home.join("credentials")).contains("old"));
    env.ok(&app, &["owner", "--add", "bob", "aa", "--registry", "mock"]);
    let sent = registry.requests();
    let put = sent.iter().find(|r| r.method == "PUT").unwrap();
    assert_eq!(
        put.headers.get("authorization").map(String::as_str),
        Some("new-token")
    );
    env.ok(&app, &["logout", "--registry", "mock"]);
    let err = env.fails(&app, &["yank", "aa@1.0.0", "--registry", "mock"]);
    assert!(err.contains("no token found"), "{err}");
    fs::write(home.join("credentials.toml"), "not = [toml").unwrap();
    let err = env.fails(&app, &["login", "--registry", "mock", "x"]);
    assert!(err.contains("failed to parse"), "{err}");
}

#[test]
fn install_refuses_foreign_files_and_offline_downloads() {
    let env = Env::new();
    let registry = serve(vec![Published {
        files: vec![("src/main.rs", "fn main() {}\n".to_string())],
        ..version("tool", "1.0.0")
    }]);
    let app = env.app(&registry, "");
    with_named_registry(&app, &registry);
    let root = env.path("root");
    fs::create_dir_all(root.join("bin")).unwrap();
    fs::write(root.join("bin/tool"), "someone else's").unwrap();
    let err = env.fails(
        &app,
        &[
            "install",
            "tool",
            "--registry",
            "mock",
            "--root",
            root.to_str().unwrap(),
        ],
    );
    assert!(err.contains("already exists in destination"), "{err}");
    assert_eq!(read(root.join("bin/tool")), "someone else's");

    let before = registry
        .requests()
        .iter()
        .filter(|r| r.path.contains("/dl/"))
        .count();
    let err = env.fails(
        &app,
        &[
            "install",
            "tool",
            "--registry",
            "mock",
            "--offline",
            "--root",
            env.path("r2").to_str().unwrap(),
        ],
    );
    assert!(err.contains("offline") || err.contains("cached"), "{err}");
    let after = registry
        .requests()
        .iter()
        .filter(|r| r.path.contains("/dl/"))
        .count();
    assert_eq!(before, after, "--offline downloads nothing");

    fs::write(root.join(".crates.toml"), "[v1\n").unwrap();
    let err = env.fails(
        &app,
        &["install", "--list", "--root", root.to_str().unwrap()],
    );
    assert!(err.contains("failed to parse"), "{err}");
}

#[test]
fn vendor_replaces_alternative_registries_and_clean_stays_in_its_directory() {
    let env = Env::new();
    let registry = serve(vec![version("aa", "1.0.0")]);
    let app = env.app(
        &registry,
        "[dependencies]\naa = { version = \"1\", registry = \"mock\" }\n",
    );
    with_named_registry(&app, &registry);
    let (out, _) = env.ok(&app, &["vendor"]);
    assert!(
        out.contains(&format!(
            "[source.\"{}\"]\nregistry = \"{}\"\nreplace-with = \"vendored-sources\"",
            registry.index_url(),
            registry.index_url()
        )),
        "{out}"
    );

    env.ok(&app, &["build", "-q"]);
    fs::create_dir_all(env.path("outside/doc")).unwrap();
    let err = env.fails(&app, &["clean", "--doc", "--target", "../../outside"]);
    assert!(err.contains("not a target triple"), "{err}");
    assert!(env.path("outside/doc").exists());
}

#[test]
fn update_locked_passes_when_nothing_changes_and_test_selectors_union() {
    let env = Env::new();
    let registry = serve(vec![version("aa", "1.0.0")]);
    let app = env.app(&registry, "[dependencies]\naa = \"1\"\n");
    env.ok(&app, &["generate-lockfile"]);
    let lock = read(app.join("Cargo.lock"));
    env.ok(&app, &["update", "--locked"]);
    assert_eq!(read(app.join("Cargo.lock")), lock);

    fs::create_dir_all(app.join("examples")).unwrap();
    fs::write(
        app.join("examples/plain.rs"),
        "fn main() {}\n#[test]\nfn plain_example_test() {}\n",
    )
    .unwrap();
    let (out, err) = env.ok(&app, &["test", "--tests", "--examples"]);
    assert!(
        format!("{out}{err}").contains("plain_example_test"),
        "{out}{err}"
    );

    fs::create_dir_all(app.join("benches")).unwrap();
    fs::write(app.join("benches/b.rs"), "fn main() {}\n").unwrap();
    fs::write(
        app.join("Cargo.toml"),
        read(app.join("Cargo.toml")) + "\n[[bench]]\nname = \"b\"\nharness = false\n",
    )
    .unwrap();
    env.ok(&app, &["bench", "--profile", "bench", "--no-run"]);

    let out_dir = env.path("artifacts");
    env.ok(
        &app,
        &[
            "build",
            "--message-format",
            "json",
            "--example",
            "plain",
            "-Z",
            "unstable-options",
            "--artifact-dir",
            out_dir.to_str().unwrap(),
        ],
    );
    assert!(
        out_dir.join("plain").is_file(),
        "example copied in JSON mode"
    );
}

#[test]
fn doc_redocuments_when_the_features_change_back() {
    let env = Env::new();
    let registry = serve(Vec::new());
    let app = env.app(&registry, "[features]\nextra = []\n");
    fs::write(
        app.join("src/lib.rs"),
        "#[cfg(feature = \"extra\")]\npub fn only_with_extra() {}\npub fn always() {}\n",
    )
    .unwrap();
    env.ok(&app, &["doc", "--no-deps", "-q"]);
    env.ok(&app, &["doc", "--no-deps", "-q", "--features", "extra"]);
    assert!(app.join("target/doc/app/fn.only_with_extra.html").is_file());
    let (_, err) = env.ok(&app, &["doc", "--no-deps"]);
    assert!(
        err.contains("Documenting app"),
        "docs of the other features are not current: {err}"
    );
}

fn host_triple() -> String {
    let out = Command::new("rustc").arg("-vV").output().unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find_map(|l| l.strip_prefix("host: ").map(str::to_string))
        .unwrap()
}

#[test]
fn clean_handles_targets_dry_runs_and_custom_binary_names() {
    let env = Env::new();
    let registry = serve(Vec::new());
    let app = env.app(
        &registry,
        "[[bin]]\nname = \"my-tool\"\npath = \"src/main.rs\"\n",
    );
    // A dry run on a project that never built leaves no target directory behind.
    let (_, err) = env.ok(&app, &["clean", "--dry-run"]);
    assert!(err.contains("Summary 0 files"), "{err}");
    assert!(!app.join("target").exists());

    let host = host_triple();
    env.ok(&app, &["build", "-q", "--target", &host]);
    assert!(app
        .join("target")
        .join(&host)
        .join("debug/my-tool")
        .is_file());
    env.ok(&app, &["clean", "--target", &host, "-p", "app"]);
    assert!(!app
        .join("target")
        .join(&host)
        .join("debug/my-tool")
        .exists());

    env.ok(&app, &["build", "-q"]);
    assert!(app.join("target/debug/my-tool").is_file());
    env.ok(&app, &["clean", "-p", "app"]);
    assert!(
        !app.join("target/debug/my-tool").exists(),
        "the package's custom-named binary goes too"
    );
}

#[test]
fn an_index_moved_by_the_environment_gets_no_stored_token() {
    let env = Env::new();
    let registry = serve(Vec::new());
    let app = env.app(&registry, "");
    with_named_registry(&app, &registry);
    env.ok(&app, &["login", "--registry", "mock", "stored"]);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(env.path("cargo-home/credentials.toml"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o077, 0, "credentials are private: {mode:o}");
    }
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mitts"));
    cmd.args([
        "owner",
        "--add",
        "bob",
        "aa",
        "--index",
        &registry.index_url(),
    ])
    .current_dir(&app)
    .env("CARGO_HOME", env.path("cargo-home"))
    .env("MITTS_CACHE_DIR", env.path("cache"))
    .env(
        "CARGO_REGISTRIES_MOCK_INDEX",
        "sparse+http://127.0.0.1:9/elsewhere/",
    );
    let out = cmd.output().unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "{err}");
    assert!(err.contains("pass its token with `--token`"), "{err}");
    assert!(
        !registry
            .requests()
            .iter()
            .any(|r| r.headers.contains_key("authorization")),
        "no token was sent"
    );
}

#[test]
fn install_from_an_index_and_keeps_records_whole_on_errors() {
    let env = Env::new();
    let registry = serve(vec![Published {
        files: vec![("src/main.rs", "fn main() {}\n".to_string())],
        ..version("tool", "1.0.0")
    }]);
    let app = env.app(&registry, "");
    let root = env.path("root");
    let root_s = root.to_str().unwrap();
    env.ok(
        &app,
        &[
            "install",
            "tool",
            "--index",
            &registry.index_url(),
            "--root",
            root_s,
        ],
    );
    let records = read(root.join(".crates.toml"));
    assert!(
        records.contains(&format!("tool 1.0.0 ({})", registry.index_url())),
        "{records}"
    );

    fs::write(root.join(".crates2.json"), "{not json").unwrap();
    env.fails(&app, &["uninstall", "tool", "--root", root_s]);
    assert_eq!(
        read(root.join(".crates.toml")),
        records,
        "v1 records untouched"
    );
    assert!(root.join("bin/tool").is_file());
}

#[test]
fn metadata_no_deps_reports_cargos_default_members() {
    let env = Env::new();
    let registry = serve(Vec::new());
    let app = env.app(&registry, "[workspace]\nmembers = [\"crates/*\"]\n");
    fs::create_dir_all(app.join("crates/extra/src")).unwrap();
    fs::write(
        app.join("crates/extra/Cargo.toml"),
        "[package]\nname = \"extra\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(app.join("crates/extra/src/lib.rs"), "").unwrap();
    let (out, _) = env.ok(&app, &["metadata", "--no-deps", "--format-version", "1"]);
    let doc: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(doc["workspace_members"].as_array().unwrap().len(), 2);
    let defaults = doc["workspace_default_members"].as_array().unwrap();
    assert_eq!(defaults.len(), 1, "{doc}");
    // Cargo's package ID spec leaves the name out when the directory already is the name.
    assert!(
        defaults[0].as_str().unwrap().ends_with("/app#0.1.0"),
        "{doc}"
    );
}

#[test]
fn clean_of_an_unbuilt_target_and_corrupt_records_are_handled() {
    let env = Env::new();
    let registry = serve(Vec::new());
    let app = env.app(&registry, "");
    env.ok(&app, &["build", "-q"]);
    // A target that was never built is a successful no-op.
    let (_, err) = env.ok(&app, &["clean", "--target", "wasm32-unknown-unknown"]);
    assert!(err.contains("Removed 0 files"), "{err}");
    assert!(!app.join("target/wasm32-unknown-unknown").exists());

    let root = env.path("root");
    fs::create_dir_all(&root).unwrap();
    fs::write(
        root.join(".crates.toml"),
        "[v1]\n\"tool 1.0.0 (x)\" = \"not a list\"\n",
    )
    .unwrap();
    let err = env.fails(
        &app,
        &["install", "--list", "--root", root.to_str().unwrap()],
    );
    assert!(err.contains("is corrupt"), "{err}");

    // A binary and an example with one name cannot share the artifact directory.
    fs::create_dir_all(app.join("examples")).unwrap();
    fs::write(app.join("examples/app.rs"), "fn main() {}\n").unwrap();
    let out_dir = env.path("artifacts");
    let err = env.fails(
        &app,
        &[
            "build",
            "-q",
            "--bins",
            "--examples",
            "-Z",
            "unstable-options",
            "--artifact-dir",
            out_dir.to_str().unwrap(),
        ],
    );
    assert!(err.contains("two files named `app`"), "{err}");
}

#[test]
fn corrupt_credentials_or_records_change_nothing() {
    let env = Env::new();
    let registry = serve(Vec::new());
    let app = env.app(&registry, "");
    with_named_registry(&app, &registry);
    let home = env.path("cargo-home");
    fs::create_dir_all(&home).unwrap();
    fs::write(home.join("credentials"), "not = [toml").unwrap();
    let err = env.fails(&app, &["login", "--registry", "mock", "new"]);
    assert!(err.contains("failed to parse"), "{err}");
    assert!(
        !home.join("credentials.toml").exists(),
        "nothing written before both files were read"
    );

    let root = env.path("root");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join(".crates2.json"), "{\"installs\": []}").unwrap();
    let err = env.fails(&app, &["uninstall", "x", "--root", root.to_str().unwrap()]);
    assert!(
        err.contains("is corrupt") || err.contains("did not match"),
        "{err}"
    );
}

#[test]
fn json_build_reports_a_failed_artifact_export_once() {
    let env = Env::new();
    let registry = serve(Vec::new());
    let app = env.app(&registry, "");
    fs::create_dir_all(app.join("examples")).unwrap();
    fs::write(app.join("examples/app.rs"), "fn main() {}\n").unwrap();
    let out_dir = env.path("artifacts");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mitts"));
    cmd.args([
        "build",
        "--bins",
        "--examples",
        "--message-format",
        "json",
        "-Z",
        "unstable-options",
        "--artifact-dir",
        out_dir.to_str().unwrap(),
    ])
    .current_dir(&app)
    .env("CARGO_HOME", env.path("cargo-home"))
    .env("MITTS_CACHE_DIR", env.path("cache"));
    let out = cmd.output().unwrap();
    assert_eq!(out.status.code(), Some(101));
    let stdout = String::from_utf8_lossy(&out.stdout);
    let finished: Vec<&str> = stdout
        .lines()
        .filter(|l| l.contains("\"build-finished\""))
        .collect();
    assert_eq!(
        finished,
        ["{\"reason\":\"build-finished\",\"success\":false}"],
        "{stdout}"
    );
}

#[test]
fn build_scripts_write_into_scratch_out_dir_per_target_directory() {
    let env = Env::new();
    // dtolnay's `scratch`: the library hands out directories under its own OUT_DIR.
    let registry = serve(vec![Published {
        files: vec![
            ("build.rs", "fn main() {}\n".to_string()),
            (
                "src/lib.rs",
                "use std::path::{Path, PathBuf};\n\
                 pub fn path(name: &str) -> PathBuf {\n\
                 \x20   let p = Path::new(env!(\"OUT_DIR\")).join(name);\n\
                 \x20   std::fs::create_dir_all(&p).unwrap();\n\
                 \x20   p\n\
                 }\n"
                .to_string(),
            ),
        ],
        ..version("scratch", "1.0.7")
    }]);
    // Like cxx-build: a build script that writes into scratch's directory.
    let build_rs = "fn main() {\n\
                    \x20   let dir = scratch::path(\"cxxbridge\");\n\
                    \x20   std::fs::write(dir.join(\"generated.h\"), \"// header\\n\").unwrap();\n\
                    \x20   println!(\"cargo:rustc-env=SCRATCH_DIR={}\", dir.display());\n\
                    }\n";
    let main_rs = "fn main() { println!(\"{}\", env!(\"SCRATCH_DIR\")); }\n";
    let make = |name: &str| {
        let app = env.app(&registry, "[build-dependencies]\nscratch = \"1\"\n");
        let dir = env.path(name);
        fs::rename(&app, &dir).unwrap();
        fs::write(dir.join("build.rs"), build_rs).unwrap();
        fs::write(dir.join("src/main.rs"), main_rs).unwrap();
        dir
    };
    let first = make("first");
    let (out, _) = env.ok(&first, &["run", "-q"]);
    let scratch_dir = std::path::PathBuf::from(out.trim());
    // Compared canonically: on macOS the temp dir is /var/..., which is /private/var/....
    let under = |path: &std::path::Path, dir: &std::path::Path| {
        path.canonicalize()
            .unwrap()
            .starts_with(dir.join("target").canonicalize().unwrap())
    };
    assert!(
        under(&scratch_dir, &first),
        "scratch's OUT_DIR is in the project's target directory: {}",
        scratch_dir.display()
    );
    assert!(scratch_dir.join("generated.h").is_file());
    // OUT_DIR holds only what build scripts put there: Mitts keeps its records beside it.
    let out_dir = scratch_dir.parent().unwrap();
    let names: Vec<String> = fs::read_dir(out_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["cxxbridge"], "{}", out_dir.display());
    // `clean` removes the directory with what was written there, as Cargo's clean does.
    env.ok(&first, &["clean"]);
    assert!(!scratch_dir.exists(), "{}", scratch_dir.display());
    // Built again from the cache: the script that wrote there runs again, so its file is back.
    let (again, _) = env.ok(&first, &["run", "-q"]);
    assert_eq!(again.trim(), out.trim());
    assert!(scratch_dir.join("generated.h").is_file());

    // A second project with the same cache gets its own directory.
    let second = make("second");
    let (other, _) = env.ok(&second, &["run", "-q"]);
    assert!(
        under(std::path::Path::new(other.trim()), &second),
        "{other}"
    );
    assert!(std::path::Path::new(other.trim())
        .join("generated.h")
        .is_file());
}

#[cfg(unix)]
#[test]
fn external_subcommands_and_list_work_like_cargo() {
    use std::os::unix::fs::PermissionsExt;
    let env = Env::new();
    let bin = env.path("tools");
    fs::create_dir_all(&bin).unwrap();
    let tool = bin.join("cargo-hello");
    fs::write(
        &tool,
        "#!/bin/sh\necho \"args: $*\"\necho \"cargo: $CARGO\"\nexit 3\n",
    )
    .unwrap();
    fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_mitts"))
            .args(args)
            .env("PATH", &path)
            .env("CARGO_HOME", env.path("cargo-home"))
            .output()
            .unwrap()
    };

    // `mitts hello a b` runs `cargo-hello hello a b` with CARGO pointing at Mitts.
    let out = run(&["hello", "a", "--flag"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        out.status.code(),
        Some(3),
        "the tool's exit code is returned"
    );
    assert!(stdout.contains("args: hello a --flag"), "{stdout}");
    let cargo_line = stdout.lines().find(|l| l.starts_with("cargo: ")).unwrap();
    assert!(cargo_line.ends_with("mitts"), "{cargo_line}");

    // An unknown command is Cargo's error, with a suggestion for a typo.
    let out = run(&["biuld"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(101));
    assert!(stderr.contains("no such command: `biuld`"), "{stderr}");
    assert!(stderr.contains("Did you mean `build`?"), "{stderr}");

    // `--list` names built-in commands, the built-in aliases and the external tool.
    let out = run(&["--list"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success());
    assert!(stdout.starts_with("Installed Commands:"), "{stdout}");
    for word in [
        "    build",
        "    b                    alias: build",
        "    hello",
    ] {
        assert!(
            stdout.lines().any(|l| l.starts_with(word)),
            "{word}: {stdout}"
        );
    }
}

#[cfg(unix)]
#[test]
fn new_sets_up_mercurial_pijul_and_fossil_like_cargo() {
    use std::os::unix::fs::PermissionsExt;
    let env = Env::new();
    let bin = env.path("vcs-bin");
    fs::create_dir_all(&bin).unwrap();
    // Stand-ins that do what the real tools do to the file system.
    for (tool, script) in [
        ("hg", "#!/bin/sh\n[ \"$1\" = init ] && mkdir -p \"$2/.hg\"\n"),
        ("pijul", "#!/bin/sh\n[ \"$1\" = init ] && mkdir -p .pijul\n"),
        (
            "fossil",
            "#!/bin/sh\nif [ \"$1\" = init ]; then : > \"$3\"; fi\nif [ \"$1\" = open ]; then : > .fslckout; fi\n",
        ),
    ] {
        let path = bin.join(tool);
        fs::write(&path, script).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let work = env.path("projects");
    fs::create_dir_all(&work).unwrap();
    let new = |vcs: &str, name: &str, path_var: &str| {
        Command::new(env!("CARGO_BIN_EXE_mitts"))
            .args(["new", "--vcs", vcs, name])
            .current_dir(&work)
            .env("PATH", path_var)
            .env("CARGO_HOME", env.path("cargo-home"))
            .output()
            .unwrap()
    };

    assert!(new("hg", "with-hg", &path).status.success());
    assert!(work.join("with-hg/.hg").is_dir());
    assert_eq!(read(work.join("with-hg/.hgignore")), "^target$\n");

    assert!(new("pijul", "with-pijul", &path).status.success());
    assert!(work.join("with-pijul/.pijul").is_dir());
    assert_eq!(read(work.join("with-pijul/.ignore")), "/target\n");

    assert!(new("fossil", "with-fossil", &path).status.success());
    assert!(work.join("with-fossil/.fossil").is_file());
    assert!(work.join("with-fossil/.fslckout").is_file());
    assert_eq!(
        read(work.join("with-fossil/.fossil-settings/ignore-glob")),
        "target\n"
    );
    assert_eq!(
        read(work.join("with-fossil/.fossil-settings/clean-glob")),
        "target\n"
    );

    // A missing tool is an error, as in Cargo.
    let out = new("hg", "no-hg", "/nonexistent");
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("could not run `hg`"));
}

#[test]
fn resolver_3_prefers_versions_the_projects_rust_version_supports() {
    let env = Env::new();
    let registry = serve(vec![
        Published {
            rust_version: Some("1.60"),
            ..version("aa", "1.0.0")
        },
        Published {
            rust_version: Some("99.0"),
            ..version("aa", "1.1.0")
        },
    ]);
    // resolver = "3" means incompatible-rust-versions = "fallback"; the limit is the
    // package's rust-version.
    let app = env.app(&registry, "[dependencies]\naa = \"1\"\n");
    let manifest = read(app.join("Cargo.toml")).replace(
        "edition = \"2021\"",
        "edition = \"2021\"\nrust-version = \"1.70\"\nresolver = \"3\"",
    );
    fs::write(app.join("Cargo.toml"), manifest).unwrap();
    env.ok(&app, &["generate-lockfile"]);
    assert!(read(app.join("Cargo.lock")).contains("name = \"aa\"\nversion = \"1.0.0\""));

    // The configuration key wins: `allow` takes the highest version.
    env.ok(
        &app,
        &[
            "generate-lockfile",
            "--config",
            "resolver.incompatible-rust-versions=\"allow\"",
        ],
    );
    assert!(read(app.join("Cargo.lock")).contains("name = \"aa\"\nversion = \"1.1.0\""));
}

#[cfg(unix)]
#[test]
fn vcs_setup_keeps_existing_files_and_finishes_a_half_done_fossil() {
    use std::os::unix::fs::PermissionsExt;
    let env = Env::new();
    let bin = env.path("vcs-bin2");
    fs::create_dir_all(&bin).unwrap();
    // fossil `open` into a non-empty directory needs --force, like the real tool.
    let fossil = "#!/bin/sh\nif [ \"$1\" = init ]; then : > \"$3\"; fi\nif [ \"$1\" = open ]; then [ \"$2\" = --force ] || exit 1; : > .fslckout; fi\n";
    let hg = "#!/bin/sh\n[ \"$1\" = init ] && mkdir -p \"$2/.hg\"\n";
    for (tool, script) in [("fossil", fossil), ("hg", hg)] {
        let path = bin.join(tool);
        fs::write(&path, script).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let init = |dir: &std::path::Path, vcs: &str| {
        Command::new(env!("CARGO_BIN_EXE_mitts"))
            .args(["init", "--vcs", vcs])
            .current_dir(dir)
            .env("PATH", &path)
            .env("CARGO_HOME", env.path("cargo-home"))
            .output()
            .unwrap()
    };

    // Fossil into a directory with files, and a repository whose checkout was never opened.
    let fossil_dir = env.path("fossil-proj");
    fs::create_dir_all(&fossil_dir).unwrap();
    fs::write(fossil_dir.join("notes.txt"), "keep\n").unwrap();
    fs::write(fossil_dir.join(".fossil"), "").unwrap();
    let out = init(&fossil_dir, "fossil");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        fossil_dir.join(".fslckout").is_file(),
        "the missing checkout was opened"
    );
    assert_eq!(read(fossil_dir.join("notes.txt")), "keep\n");

    // An .hgignore that ends in glob syntax gets `syntax: regexp` before `^target$`.
    let hg_dir = env.path("hg-proj");
    fs::create_dir_all(&hg_dir).unwrap();
    fs::write(hg_dir.join(".hgignore"), "syntax: glob\n*.o\n").unwrap();
    assert!(init(&hg_dir, "hg").status.success());
    assert_eq!(
        read(hg_dir.join(".hgignore")),
        "syntax: glob\n*.o\nsyntax: regexp\n^target$\n"
    );

    // An ignore file that is not text stops the command and stays as it was.
    let bad_dir = env.path("hg-bad");
    fs::create_dir_all(&bad_dir).unwrap();
    fs::write(bad_dir.join(".hgignore"), [0xff, 0xfe, b'\n']).unwrap();
    let out = init(&bad_dir, "hg");
    assert!(!out.status.success());
    assert_eq!(
        fs::read(bad_dir.join(".hgignore")).unwrap(),
        [0xff, 0xfe, b'\n']
    );
}

#[test]
fn tree_features_shows_requested_features_and_prunes_them_with_their_package() {
    let env = Env::new();
    let registry = serve(vec![Published {
        features: "{\"default\":[\"std\"],\"std\":[]}",
        // The crate's own manifest declares them too (metadata reads it).
        manifest_extra: "[features]\ndefault = [\"std\"]\nstd = []\n",
        ..version("aa", "1.0.0")
    }]);
    let app = env.app(&registry, "[dependencies]\naa = \"1\"\n");
    let (out, _) = env.ok(&app, &["tree", "-e", "features"]);
    assert!(out.contains("── aa feature \"default\"\n"), "{out}");
    assert!(out.contains("── aa feature \"std\"\n"), "{out}");
    assert!(out.contains("── aa v1.0.0"), "{out}");
    let (out, _) = env.ok(&app, &["tree", "-e", "features", "--prune", "aa"]);
    assert!(!out.contains("aa"), "{out}");
}

#[test]
fn msrv_limit_counts_path_members_and_falls_back_when_nothing_fits() {
    let env = Env::new();
    let registry = serve(vec![
        Published {
            rust_version: Some("1.40"),
            ..version("aa", "1.0.0")
        },
        Published {
            rust_version: Some("1.60"),
            ..version("aa", "1.1.0")
        },
        // bb 1.0.0 fits the limit but needs a crate that does not exist.
        Published {
            rust_version: Some("1.40"),
            deps: vec![dep("missing", "=9")],
            ..version("bb", "1.0.0")
        },
        Published {
            rust_version: Some("99.0"),
            ..version("bb", "1.1.0")
        },
    ]);
    // The root says 1.70; an in-tree path dependency (an implicit member) says 1.50.
    let app = env.app(
        &registry,
        "[dependencies]\naa = \"1\"\nbb = \"1\"\nlow = { path = \"crates/low\" }\n\n[workspace]\nresolver = \"3\"\n",
    );
    let manifest = read(app.join("Cargo.toml")).replace(
        "edition = \"2021\"",
        "edition = \"2021\"\nrust-version = \"1.70\"",
    );
    fs::write(app.join("Cargo.toml"), manifest).unwrap();
    fs::create_dir_all(app.join("crates/low/src")).unwrap();
    fs::write(
        app.join("crates/low/Cargo.toml"),
        "[package]\nname = \"low\"\nversion = \"0.1.0\"\nedition = \"2021\"\nrust-version = \"1.50\"\n",
    )
    .unwrap();
    fs::write(app.join("crates/low/src/lib.rs"), "").unwrap();
    env.ok(&app, &["generate-lockfile"]);
    let lock = read(app.join("Cargo.lock"));
    // No version of bb that fits 1.50 has a working graph, so the limit is relaxed for bb
    // alone; aa keeps the version that fits.
    assert!(
        lock.contains("name = \"bb\"\nversion = \"1.1.0\""),
        "{lock}"
    );
    assert!(
        lock.contains("name = \"aa\"\nversion = \"1.0.0\""),
        "{lock}"
    );
}

#[test]
fn msrv_limit_counts_in_tree_path_members() {
    let env = Env::new();
    let registry = serve(vec![
        Published {
            rust_version: Some("1.40"),
            ..version("aa", "1.0.0")
        },
        Published {
            rust_version: Some("1.60"),
            ..version("aa", "1.1.0")
        },
    ]);
    let app = env.app(
        &registry,
        "[dependencies]\naa = \"1\"\nlow = { path = \"crates/low\" }\n\n[workspace]\nresolver = \"3\"\n",
    );
    let manifest = read(app.join("Cargo.toml")).replace(
        "edition = \"2021\"",
        "edition = \"2021\"\nrust-version = \"1.70\"",
    );
    fs::write(app.join("Cargo.toml"), manifest).unwrap();
    fs::create_dir_all(app.join("crates/low/src")).unwrap();
    fs::write(
        app.join("crates/low/Cargo.toml"),
        "[package]\nname = \"low\"\nversion = \"0.1.0\"\nedition = \"2021\"\nrust-version = \"1.50\"\n",
    )
    .unwrap();
    fs::write(app.join("crates/low/src/lib.rs"), "").unwrap();
    env.ok(&app, &["generate-lockfile"]);
    let lock = read(app.join("Cargo.lock"));
    assert!(
        lock.contains("name = \"aa\"\nversion = \"1.0.0\""),
        "1.50 from the path member: {lock}"
    );
}

#[test]
fn metadata_names_the_package_and_the_rename_like_cargo() {
    let env = Env::new();
    let registry = serve(vec![Published {
        manifest_extra: "[features]\nsmall = []\nstd = []\n",
        features: "{\"small\":[],\"std\":[]}",
        ..version("aa", "1.0.0")
    }]);
    let app = env.app(
        &registry,
        "[dependencies]\nold = { package = \"aa\", version = \"1\", default-features = false, features = [\"small\"] }\n",
    );
    let (out, _) = env.ok(&app, &["metadata", "--format-version", "1", "--no-deps"]);
    let doc: serde_json::Value = serde_json::from_str(&out).unwrap();
    let dep = &doc["packages"][0]["dependencies"][0];
    assert_eq!(dep["name"], "aa", "{dep}");
    assert_eq!(dep["rename"], "old", "{dep}");
    // The feature graph follows the renamed declaration.
    let (out, _) = env.ok(&app, &["tree", "-e", "features"]);
    assert!(out.contains("── aa feature \"small\"\n"), "{out}");
    assert!(!out.contains("aa feature \"std\""), "{out}");
}

#[cfg(unix)]
#[test]
fn hg_and_fossil_setup_respect_what_is_there() {
    use std::os::unix::fs::PermissionsExt;
    let env = Env::new();
    let bin = env.path("vcs-bin3");
    fs::create_dir_all(&bin).unwrap();
    // A stand-in that records every call: init/open must not run in an existing checkout.
    let log = env.path("fossil-calls");
    let fossil = format!("#!/bin/sh\necho \"$@\" >> {}\n", log.display());
    let hg = "#!/bin/sh\n[ \"$1\" = init ] && mkdir -p \"$2/.hg\"\n".to_string();
    for (tool, script) in [("fossil", fossil), ("hg", hg)] {
        let path = bin.join(tool);
        fs::write(&path, script).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let init = |dir: &std::path::Path, vcs: &str| {
        Command::new(env!("CARGO_BIN_EXE_mitts"))
            .args(["init", "--vcs", vcs])
            .current_dir(dir)
            .env("PATH", &path)
            .env("CARGO_HOME", env.path("cargo-home"))
            .output()
            .unwrap()
    };
    let checkout = env.path("fossil-checkout");
    fs::create_dir_all(&checkout).unwrap();
    fs::write(checkout.join(".fslckout"), "").unwrap();
    assert!(init(&checkout, "fossil").status.success());
    assert!(!checkout.join(".fossil").exists(), "no second repository");
    assert!(!log.exists(), "fossil was not run in an existing checkout");

    // A `^target$` written under glob syntax does not count: a regexp section is added.
    let hg_dir = env.path("hg-glob-rule");
    fs::create_dir_all(&hg_dir).unwrap();
    fs::write(hg_dir.join(".hgignore"), "syntax: glob\n^target$\n").unwrap();
    assert!(init(&hg_dir, "hg").status.success());
    assert_eq!(
        read(hg_dir.join(".hgignore")),
        "syntax: glob\n^target$\nsyntax: regexp\n^target$\n"
    );
}

#[test]
fn msrv_limit_skips_packages_under_an_excluded_path() {
    let env = Env::new();
    let registry = serve(vec![
        Published {
            rust_version: Some("1.40"),
            ..version("aa", "1.0.0")
        },
        Published {
            rust_version: Some("1.60"),
            ..version("aa", "1.1.0")
        },
    ]);
    let app = env.app(
        &registry,
        "[dependencies]\naa = \"1\"\nlegacy-one = { path = \"crates/legacy-one\" }\n\n[workspace]\nresolver = \"3\"\nexclude = [\"crates\"]\n",
    );
    let manifest = read(app.join("Cargo.toml")).replace(
        "edition = \"2021\"",
        "edition = \"2021\"\nrust-version = \"1.70\"",
    );
    fs::write(app.join("Cargo.toml"), manifest).unwrap();
    fs::create_dir_all(app.join("crates/legacy-one/src")).unwrap();
    fs::write(
        app.join("crates/legacy-one/Cargo.toml"),
        "[package]\nname = \"legacy-one\"\nversion = \"0.1.0\"\nedition = \"2021\"\nrust-version = \"1.50\"\n",
    )
    .unwrap();
    fs::write(app.join("crates/legacy-one/src/lib.rs"), "").unwrap();
    env.ok(&app, &["generate-lockfile"]);
    let lock = read(app.join("Cargo.lock"));
    assert!(
        lock.contains("name = \"aa\"\nversion = \"1.1.0\""),
        "the excluded crate's 1.50 does not count: {lock}"
    );
}

#[test]
fn resolver_backtracks_past_a_version_whose_dependency_does_not_exist() {
    let env = Env::new();
    let registry = serve(vec![
        version("cc", "1.0.0"),
        Published {
            deps: vec![dep("missing", "^1")],
            ..version("cc", "1.1.0")
        },
    ]);
    // No rust-version, resolver 2: plain backtracking, as Cargo does.
    let app = env.app(&registry, "[dependencies]\ncc = \"1\"\n");
    env.ok(&app, &["generate-lockfile"]);
    let lock = read(app.join("Cargo.lock"));
    assert!(
        lock.contains("name = \"cc\"\nversion = \"1.0.0\""),
        "{lock}"
    );
}

#[test]
fn resolver_backtracks_into_another_branch_when_the_first_guess_dead_ends() {
    let env = Env::new();
    // aa 1.1 and bb 1.1 pin incompatible xx versions. Ruling out aa 1.1 fails at the root
    // (`=1.1.0`); the graph resolves with bb 1.0, as under Cargo.
    let registry = serve(vec![
        version("xx", "1.0.0"),
        version("xx", "1.1.0"),
        version("aa", "1.0.0"),
        Published {
            deps: vec![dep("xx", "=1.0.0")],
            ..version("aa", "1.1.0")
        },
        Published {
            deps: vec![dep("xx", "^1")],
            ..version("bb", "1.0.0")
        },
        Published {
            deps: vec![dep("xx", "=1.1.0")],
            ..version("bb", "1.1.0")
        },
    ]);
    let app = env.app(&registry, "[dependencies]\naa = \"=1.1.0\"\nbb = \"1\"\n");
    env.ok(&app, &["generate-lockfile"]);
    let lock = read(app.join("Cargo.lock"));
    for (name, version) in [("aa", "1.1.0"), ("bb", "1.0.0"), ("xx", "1.0.0")] {
        assert!(
            lock.contains(&format!("name = \"{name}\"\nversion = \"{version}\"")),
            "{name} {version}: {lock}"
        );
    }
}

#[test]
fn resolver_backtracks_from_a_request_the_selected_patch_cannot_meet() {
    let env = Env::new();
    // ff is patched to 1.0.0; bb 1.1 needs ff ^1.1, which the patch cannot be. Cargo settles
    // on bb 1.0, which takes the patched ff.
    let registry = serve(vec![
        version("ff", "1.0.0"),
        version("ff", "1.1.0"),
        Published {
            deps: vec![dep("ff", "^1")],
            ..version("bb", "1.0.0")
        },
        Published {
            deps: vec![dep("ff", "^1.1")],
            ..version("bb", "1.1.0")
        },
    ]);
    let app = env.app(
        &registry,
        "[dependencies]\nff = \"1\"\nbb = \"1\"\n\n[patch.crates-io]\nff = { path = \"ff\" }\n",
    );
    fs::create_dir_all(app.join("ff/src")).unwrap();
    fs::write(
        app.join("ff/Cargo.toml"),
        "[package]\nname = \"ff\"\nversion = \"1.0.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(app.join("ff/src/lib.rs"), "").unwrap();
    env.ok(&app, &["generate-lockfile"]);
    let lock = read(app.join("Cargo.lock"));
    assert!(
        lock.contains("name = \"bb\"\nversion = \"1.0.0\""),
        "{lock}"
    );
    assert!(
        lock.contains("name = \"ff\"\nversion = \"1.0.0\"")
            && !lock.contains("name = \"ff\"\nversion = \"1.1.0\""),
        "{lock}"
    );
}

#[test]
fn metadata_names_an_inherited_path_alias_like_cargo() {
    let env = Env::new();
    let registry = serve(vec![version("aa", "1.0.0")]);
    let app = env.app(
        &registry,
        "[dependencies]\nhelper = { workspace = true }\n\n[workspace]\n\n[workspace.dependencies]\nhelper = { path = \"util\", package = \"my-util\" }\n",
    );
    fs::create_dir_all(app.join("util/src")).unwrap();
    fs::write(
        app.join("util/Cargo.toml"),
        "[package]\nname = \"my-util\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(app.join("util/src/lib.rs"), "").unwrap();
    let (out, _) = env.ok(&app, &["metadata", "--format-version", "1"]);
    let doc: serde_json::Value = serde_json::from_str(&out).unwrap();
    let root = doc["resolve"]["root"].as_str().unwrap().to_string();
    let node = doc["resolve"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == root.as_str())
        .unwrap();
    let names: Vec<&str> = node["deps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["helper"], "{node}");
}

#[test]
fn metadata_reports_inherited_dependencies_as_cargo_normalizes_them() {
    let env = Env::new();
    let registry = serve(vec![Published {
        features: r#"{"fast":[],"small":[]}"#,
        manifest_extra: "\n[features]\nfast = []\nsmall = []\n",
        ..version("aa", "1.2.0")
    }]);
    let app = env.app(
        &registry,
        "[dependencies]\nold = { workspace = true, features = [\"small\"] }\n\n[workspace]\n\n[workspace.dependencies]\nold = { package = \"aa\", version = \"1.2\", features = [\"fast\"] }\n",
    );
    let (out, _) = env.ok(&app, &["metadata", "--format-version", "1"]);
    let doc: serde_json::Value = serde_json::from_str(&out).unwrap();
    let root = doc["resolve"]["root"].as_str().unwrap().to_string();
    let package = doc["packages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == root.as_str())
        .unwrap();
    let dep = &package["dependencies"][0];
    assert_eq!(dep["name"], "aa", "{dep}");
    assert_eq!(dep["rename"], "old", "{dep}");
    assert_eq!(dep["req"], "^1.2", "{dep}");
    let mut features: Vec<&str> = dep["features"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f.as_str().unwrap())
        .collect();
    features.sort();
    assert_eq!(features, ["fast", "small"], "{dep}");
}

#[test]
fn metadata_normalizes_a_path_dependency_against_its_own_workspace() {
    let env = Env::new();
    let registry = serve(vec![version("aa", "1.0.0")]);
    let app = env.app(
        &registry,
        "[dependencies]\next = { path = \"../other/ext\" }\n",
    );
    let other = app.parent().unwrap().join("other");
    fs::create_dir_all(other.join("ext/src")).unwrap();
    fs::write(
        other.join("Cargo.toml"),
        "[workspace]\nmembers = [\"ext\"]\n\n[workspace.dependencies]\nold = { package = \"aa\", version = \"1\" }\n",
    )
    .unwrap();
    fs::write(
        other.join("ext/Cargo.toml"),
        "[package]\nname = \"ext\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nold = { workspace = true }\n",
    )
    .unwrap();
    fs::write(other.join("ext/src/lib.rs"), "").unwrap();
    let (out, _) = env.ok(&app, &["metadata", "--format-version", "1"]);
    let doc: serde_json::Value = serde_json::from_str(&out).unwrap();
    let ext = doc["packages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "ext")
        .unwrap();
    let dep = &ext["dependencies"][0];
    assert_eq!(dep["name"], "aa", "{dep}");
    assert_eq!(dep["rename"], "old", "{dep}");
    assert_eq!(dep["req"], "^1", "{dep}");
    let node = doc["resolve"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == ext["id"])
        .unwrap();
    assert_eq!(node["deps"][0]["name"], "old", "{node}");
}

#[test]
fn metadata_keeps_an_edition_2024_members_default_features_off() {
    let env = Env::new();
    let registry = serve(vec![version("aa", "1.0.0")]);
    let app = env.app(
        &registry,
        "[dependencies]\naa = { workspace = true, default-features = false }\n\n[workspace]\n\n[workspace.dependencies]\naa = \"1\"\n",
    );
    let manifest = read(app.join("Cargo.toml")).replace("edition = \"2021\"", "edition = \"2024\"");
    fs::write(app.join("Cargo.toml"), manifest).unwrap();
    let (out, _) = env.ok(&app, &["metadata", "--format-version", "1"]);
    let doc: serde_json::Value = serde_json::from_str(&out).unwrap();
    let root = doc["resolve"]["root"].as_str().unwrap().to_string();
    let package = doc["packages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == root.as_str())
        .unwrap();
    let dep = &package["dependencies"][0];
    assert_eq!(dep["uses_default_features"], false, "{dep}");
}

#[test]
fn metadata_matches_a_patch_keyed_by_the_github_crates_io_index() {
    let env = Env::new();
    let registry = serve(vec![version("aa", "1.0.0")]);
    let app = env.app(
        &registry,
        "[dependencies]\nold = { package = \"aa\", version = \"1\" }\n\n[patch.\"https://github.com/rust-lang/crates.io-index\"]\naa = { path = \"aa\" }\n",
    );
    fs::create_dir_all(app.join("aa/src")).unwrap();
    fs::write(
        app.join("aa/Cargo.toml"),
        "[package]\nname = \"aa\"\nversion = \"1.0.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(app.join("aa/src/lib.rs"), "").unwrap();
    let (out, _) = env.ok(&app, &["metadata", "--format-version", "1"]);
    let doc: serde_json::Value = serde_json::from_str(&out).unwrap();
    let root = doc["resolve"]["root"].as_str().unwrap().to_string();
    let node = doc["resolve"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == root.as_str())
        .unwrap();
    assert_eq!(node["deps"][0]["name"], "old", "{node}");
}

#[test]
fn metadata_keeps_the_alias_of_a_registry_dependency_a_patch_replaces() {
    let env = Env::new();
    let registry = serve(vec![version("aa", "1.0.0")]);
    let app = env.app(
        &registry,
        "[dependencies]\nold = { package = \"aa\", version = \"1\" }\n\n[patch.crates-io]\naa = { path = \"aa\" }\n",
    );
    fs::create_dir_all(app.join("aa/src")).unwrap();
    fs::write(
        app.join("aa/Cargo.toml"),
        "[package]\nname = \"aa\"\nversion = \"1.0.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(app.join("aa/src/lib.rs"), "").unwrap();
    let (out, _) = env.ok(&app, &["metadata", "--format-version", "1"]);
    let doc: serde_json::Value = serde_json::from_str(&out).unwrap();
    let root = doc["resolve"]["root"].as_str().unwrap().to_string();
    let node = doc["resolve"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == root.as_str())
        .unwrap();
    let names: Vec<&str> = node["deps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["old"], "{node}");
}

#[test]
fn metadata_leaves_out_an_optional_alias_no_feature_enables() {
    let env = Env::new();
    let registry = serve(vec![version("aa", "1.0.0")]);
    let app = env.app(
        &registry,
        "[dependencies]\naa = \"1\"\nold = { package = \"aa\", version = \"1\", optional = true }\n",
    );
    let (out, _) = env.ok(&app, &["metadata", "--format-version", "1"]);
    let doc: serde_json::Value = serde_json::from_str(&out).unwrap();
    let root = doc["resolve"]["root"].as_str().unwrap().to_string();
    let node = doc["resolve"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == root.as_str())
        .unwrap();
    let names: Vec<&str> = node["deps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["aa"], "{node}");
}

#[test]
fn metadata_names_an_unrenamed_edge_by_the_library_name() {
    let env = Env::new();
    let registry = serve(vec![version("aa", "1.0.0")]);
    let app = env.app(&registry, "[dependencies]\nmy-util = { path = \"util\" }\n");
    fs::create_dir_all(app.join("util/src")).unwrap();
    fs::write(
        app.join("util/Cargo.toml"),
        "[package]\nname = \"my-util\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\nname = \"utility\"\n",
    )
    .unwrap();
    fs::write(app.join("util/src/lib.rs"), "").unwrap();
    let (out, _) = env.ok(&app, &["metadata", "--format-version", "1"]);
    let doc: serde_json::Value = serde_json::from_str(&out).unwrap();
    let root = doc["resolve"]["root"].as_str().unwrap().to_string();
    let node = doc["resolve"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == root.as_str())
        .unwrap();
    let names: Vec<&str> = node["deps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["utility"], "{node}");
    // And `tree --features` keeps the edge.
    let (tree, _) = env.ok(&app, &["tree", "-e", "features"]);
    assert!(tree.contains("my-util v0.1.0"), "{tree}");
}

#[test]
fn metadata_has_one_resolve_edge_per_alias_of_a_package() {
    let env = Env::new();
    let registry = serve(vec![version("aa", "1.0.0")]);
    let app = env.app(
        &registry,
        "[dependencies]\naa = \"1\"\nold = { package = \"aa\", version = \"1\" }\n",
    );
    let (out, _) = env.ok(&app, &["metadata", "--format-version", "1"]);
    let doc: serde_json::Value = serde_json::from_str(&out).unwrap();
    let root = doc["resolve"]["root"].as_str().unwrap().to_string();
    let node = doc["resolve"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == root.as_str())
        .unwrap();
    let mut names: Vec<&str> = node["deps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["name"].as_str().unwrap())
        .collect();
    names.sort();
    assert_eq!(names, ["aa", "old"], "{node}");
}

#[cfg(unix)]
#[test]
fn a_rooted_glob_target_rule_does_not_count_in_hgignore() {
    use std::os::unix::fs::PermissionsExt;
    let env = Env::new();
    let bin = env.path("vcs-bin4");
    fs::create_dir_all(&bin).unwrap();
    let hg = bin.join("hg");
    fs::write(&hg, "#!/bin/sh\n[ \"$1\" = init ] && mkdir -p \"$2/.hg\"\n").unwrap();
    fs::set_permissions(&hg, fs::Permissions::from_mode(0o755)).unwrap();
    let dir = env.path("hg-rooted");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(".hgignore"), "syntax: glob\n/target\n").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_mitts"))
        .args(["init", "--vcs", "hg"])
        .current_dir(&dir)
        .env(
            "PATH",
            format!(
                "{}:{}",
                bin.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .env("CARGO_HOME", env.path("cargo-home"))
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(
        read(dir.join(".hgignore")),
        "syntax: glob\n/target\nsyntax: regexp\n^target$\n"
    );
}

#[test]
fn members_matched_by_a_glob_in_a_middle_component_count_for_msrv() {
    let env = Env::new();
    let registry = serve(vec![
        Published {
            rust_version: Some("1.40"),
            ..version("aa", "1.0.0")
        },
        Published {
            rust_version: Some("1.60"),
            ..version("aa", "1.1.0")
        },
    ]);
    let app = env.app(
        &registry,
        "[dependencies]\naa = \"1\"\n\n[workspace]\nresolver = \"3\"\nmembers = [\"vendor/*/fixtures\"]\n",
    );
    let manifest = read(app.join("Cargo.toml")).replace(
        "edition = \"2021\"",
        "edition = \"2021\"\nrust-version = \"1.70\"",
    );
    fs::write(app.join("Cargo.toml"), manifest).unwrap();
    let member = app.join("vendor/one/fixtures");
    fs::create_dir_all(member.join("src")).unwrap();
    fs::write(
        member.join("Cargo.toml"),
        "[package]\nname = \"fixtures\"\nversion = \"0.1.0\"\nedition = \"2021\"\nrust-version = \"1.50\"\n",
    )
    .unwrap();
    fs::write(member.join("src/lib.rs"), "").unwrap();
    let (out, _) = env.ok(&app, &["metadata", "--no-deps", "--format-version", "1"]);
    let doc: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert!(
        doc["packages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["name"] == "fixtures"),
        "the member is found: {out}"
    );
    env.ok(&app, &["generate-lockfile"]);
    let lock = read(app.join("Cargo.lock"));
    assert!(
        lock.contains("name = \"aa\"\nversion = \"1.0.0\""),
        "the member's 1.50 is the limit: {lock}"
    );
}

/// `-p name` matching two versions is refused, as Cargo refuses it (tokio's workspace has
/// its member tokio beside a published one); `-p name@version` picks one.
#[test]
fn a_package_spec_matching_two_versions_is_ambiguous() {
    let env = Env::new();
    let root = env.path("ws");
    // The two `dup` packages live outside the workspace directory: path dependencies, not
    // members.
    let write = |rel: &str, text: &str| {
        let path = env.path(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    };
    write(
        "ws/Cargo.toml",
        "[workspace]\nmembers = [\"app\"]\nresolver = \"2\"\n",
    );
    write(
        "ws/app/Cargo.toml",
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
         [dependencies]\nold = { package = \"dup\", path = \"../../dup1\" }\n\
         new = { package = \"dup\", path = \"../../dup2\" }\n",
    );
    write(
        "ws/app/src/lib.rs",
        "pub fn f() -> u32 { old::v() + new::v() }\n",
    );
    for (dir, version) in [("dup1", "0.1.0"), ("dup2", "0.2.0")] {
        write(
            &format!("{dir}/Cargo.toml"),
            &format!("[package]\nname = \"dup\"\nversion = \"{version}\"\nedition = \"2021\"\n"),
        );
        write(&format!("{dir}/src/lib.rs"), "pub fn v() -> u32 { 1 }\n");
    }
    env.ok(&root, &["generate-lockfile"]);
    let text = env.fails(&root, &["build", "-p", "dup"]);
    assert!(
        // The report wraps long lines.
        text.contains("There are multiple `dup` packages in your project")
            && text.contains("`dup` is ambiguous.")
            && text.contains("dup@0.1.0")
            && text.contains("dup@0.2.0"),
        "{text}"
    );
    env.ok(&root, &["build", "-p", "dup@0.2.0"]);
    let text = env.fails(&root, &["build", "-p", "nope"]);
    assert!(
        text.contains("package ID specification `nope` did not match any packages"),
        "{text}"
    );
}

/// A build dependency's `rustc-link-search` reaches the link of the build script that uses
/// it, as Cargo passes it: a build dependency on windows-sys needs `windows.0.52.0.lib` from
/// windows_x86_64_msvc's directory (hyperfine's build script failed with LNK1181 on Windows).
#[test]
fn a_build_dependency_link_search_reaches_the_build_script_link() {
    let env = Env::new();
    let write = |rel: &str, text: &str| {
        let path = env.path(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    };
    write(
        "app/Cargo.toml",
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
         [build-dependencies]\nnat = { path = \"../nat\" }\n",
    );
    write("app/src/main.rs", "fn main() {}\n");
    write("app/build.rs", "fn main() { nat::touch(); }\n");
    write(
        "nat/Cargo.toml",
        "[package]\nname = \"nat\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write("nat/src/lib.rs", "pub fn touch() {}\n");
    write(
        "nat/build.rs",
        "fn main() {\n    let dir = std::path::Path::new(&std::env::var(\"OUT_DIR\").unwrap()).join(\"nat-libs\");\n    \
         std::fs::create_dir_all(&dir).unwrap();\n    \
         println!(\"cargo:rustc-link-search=native={}\", dir.display());\n}\n",
    );
    let app = env.path("app");
    env.ok(&app, &["generate-lockfile"]);
    let (out, err) = env.ok(&app, &["build", "-v"]);
    let log = format!("{out}{err}");
    let script_link = log
        .lines()
        .find(|l| l.contains("--crate-name build_script_build") && l.contains("app"))
        .unwrap_or_else(|| panic!("no build script command for app:\n{log}"));
    assert!(script_link.contains("nat-libs"), "{script_link}");
}

/// A build script is compiled only after its build dependencies' rlibs exist: a slow build
/// dependency (its own build script sleeps) must not let the consumer's build script start
/// on metadata alone. On Windows bat, starship, bottom and alacritty failed with E0463 for
/// a build dependency whose rlib appeared moments later.
#[test]
fn a_build_script_waits_for_a_slow_build_dependency() {
    let env = Env::new();
    let write = |rel: &str, text: &str| {
        let path = env.path(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    };
    write(
        "app/Cargo.toml",
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
         [dependencies]\nslow = { path = \"../slow\" }\n\n\
         [build-dependencies]\nslow = { path = \"../slow\" }\n",
    );
    write("app/src/lib.rs", "pub fn f() -> u32 { slow::v() }\n");
    write("app/build.rs", "fn main() { assert_eq!(slow::v(), 7); }\n");
    write(
        "slow/Cargo.toml",
        "[package]\nname = \"slow\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    // Big enough that codegen takes a while after the metadata is out.
    let mut body = String::from("pub fn v() -> u32 { 7 }\n");
    for i in 0..3000 {
        body.push_str(&format!(
            "pub fn f{i}(x: u64) -> u64 {{ (0..x).map(|y| y.wrapping_mul({i}) ^ (y >> 3)).sum() }}\n"
        ));
    }
    write("slow/src/lib.rs", &body);
    let app = env.path("app");
    env.ok(&app, &["generate-lockfile"]);
    env.ok(&app, &["build"]);
}
