/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! The project's own crates are built into its target directory, not the shared cache
//! (hitmeup: 70 GB of its own test executables in the shared store within two hours).
//!
//! - A workspace's library, binary and tests leave nothing in the shared store.
//! - A build that changes nothing compiles nothing: it restores from the target's store.
//! - After an edit, the target's store holds the new version only.
//! - `MITTS_SHARE_WORKSPACE_CRATES=1` stores them in the shared cache as before.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

struct Project {
    dir: TempDir,
}

impl Project {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("ws");
        fs::create_dir_all(root.join("app/src")).unwrap();
        fs::create_dir_all(root.join("core/src")).unwrap();
        fs::create_dir_all(root.join("app/tests")).unwrap();
        fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"app\", \"core\"]\nresolver = \"2\"\n",
        )
        .unwrap();
        fs::write(
            root.join("core/Cargo.toml"),
            "[package]\nname = \"core-lib\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        fs::write(
            root.join("core/src/lib.rs"),
            "pub fn answer() -> u32 { 41 }\n",
        )
        .unwrap();
        // A build script with an `OUT_DIR` file: its result is kept in the target's store.
        fs::write(
            root.join("core/build.rs"),
            "fn main() {\n    let out = std::env::var(\"OUT_DIR\").unwrap();\n    \
             std::fs::create_dir_all(format!(\"{out}/gen\")).unwrap();\n    \
             std::fs::write(format!(\"{out}/gen/x.rs\"), \"\").unwrap();\n    \
             println!(\"cargo:rerun-if-changed=build.rs\");\n}\n",
        )
        .unwrap();
        fs::write(
            root.join("app/Cargo.toml"),
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
             [dependencies]\ncore-lib = { path = \"../core\" }\n",
        )
        .unwrap();
        fs::write(
            root.join("app/src/main.rs"),
            "fn main() { println!(\"{}\", core_lib::answer()); }\n",
        )
        .unwrap();
        fs::write(
            root.join("app/tests/it.rs"),
            "#[test]\nfn answers() { assert!(core_lib::answer() > 0); }\n",
        )
        .unwrap();
        Self { dir }
    }

    fn root(&self) -> PathBuf {
        self.dir.path().join("ws")
    }

    fn cache(&self) -> PathBuf {
        self.dir.path().join("cache")
    }

    /// Runs `mitts <args>`; the `[mitts::stats]` line's `compiled=` count.
    fn mitts(&self, args: &[&str], share: bool) -> usize {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mitts"));
        cmd.args(args)
            .arg("--cache-dir")
            .arg(self.cache())
            .current_dir(self.root())
            .env("CARGO_HOME", self.dir.path().join("cargo-home"))
            .env_remove("MITTS_CACHE_DIR")
            .env_remove("RUSTC_WRAPPER")
            .env_remove("CARGO_BUILD_RUSTC_WRAPPER");
        if share {
            cmd.env("MITTS_SHARE_WORKSPACE_CRATES", "1");
        } else {
            cmd.env_remove("MITTS_SHARE_WORKSPACE_CRATES");
        }
        let out = cmd.output().unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(out.status.success(), "{stderr}");
        stderr
            .lines()
            .filter_map(|l| l.split("compiled=").nth(1))
            .filter_map(|rest| rest.split_whitespace().next()?.parse().ok())
            .next_back()
            .unwrap_or_else(|| panic!("no stats line:\n{stderr}"))
    }
}

fn files(dir: &Path) -> Vec<PathBuf> {
    fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .collect()
}

fn local_objects(p: &Project) -> Vec<PathBuf> {
    files(&p.root().join("target/debug/.mitts-store/cas/objects"))
}

/// `target/debug/deps/libcore_lib-<hash>.rlib`: the name changes with the library's key.
fn versioned_libs(p: &Project) -> Vec<PathBuf> {
    files(&p.root().join("target/debug/deps"))
        .into_iter()
        .filter(|f| {
            let name = f.file_name().unwrap().to_string_lossy().into_owned();
            name.starts_with("libcore_lib-") && name.ends_with(".rlib")
        })
        .collect()
}

/// Each entry of the target's action cache: its action id and output paths (for a failure).
fn local_entries(p: &Project) -> String {
    let mut lines: Vec<String> = files(&p.root().join("target/debug/.mitts-store/ac"))
        .into_iter()
        .filter(|f| f.extension().is_some_and(|e| e == "json"))
        .filter_map(|f| fs::read_to_string(f).ok())
        .filter_map(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .map(|v| {
            let outputs: Vec<String> = v["outputs"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|o| o["relative_path"].to_string())
                .collect();
            format!("{} {}", v["action_id"], outputs.join(" "))
        })
        .collect();
    lines.sort();
    lines.join("\n")
}

#[test]
fn the_projects_own_crates_stay_in_its_target() {
    let p = Project::new();
    assert!(p.mitts(&["build"], false) > 0);
    assert!(p.mitts(&["test", "--no-run"], false) > 0);
    assert!(
        files(&p.cache().join("cas/objects")).is_empty(),
        "the shared store holds no workspace crate: {:?}",
        files(&p.cache().join("cas/objects"))
    );
    assert!(files(&p.cache().join("cas/artifacts")).is_empty());
    let first = local_objects(&p);
    assert!(!first.is_empty(), "the target's store holds them");
    let libs_before = versioned_libs(&p);
    assert!(!libs_before.is_empty());
    let entries_before = local_entries(&p);

    // Nothing changed: nothing is compiled.
    assert_eq!(p.mitts(&["build"], false), 0);
    assert_eq!(p.mitts(&["test", "--no-run"], false), 0);

    // An edit replaces the old version in the target's store.
    fs::write(
        p.root().join("core/src/lib.rs"),
        "pub fn answer() -> u32 { 42 }\n",
    )
    .unwrap();
    assert!(p.mitts(&["build"], false) > 0);
    assert!(p.mitts(&["test", "--no-run"], false) > 0);
    let after = local_objects(&p);
    assert!(
        after.len() <= first.len(),
        "one version per unit: {} objects before the edit, {} after\n\
         entries before:\n{entries_before}\nentries after:\n{}",
        first.len(),
        after.len(),
        local_entries(&p)
    );
    assert!(
        first.iter().any(|o| !after.contains(o)),
        "the replaced versions are gone"
    );
    assert!(files(&p.cache().join("cas/objects")).is_empty());
    // The target does not grow with edits either: one library file per command that built
    // it. (Its name changes with the edit only without incremental compilation.)
    assert_eq!(
        versioned_libs(&p).len(),
        libs_before.len(),
        "{:?} before the edit, {:?} after",
        libs_before,
        versioned_libs(&p)
    );
    assert!(
        p.root()
            .join("target/debug/.mitts-store/cas/build_scripts")
            .is_dir(),
        "the build script's result is in the target's store"
    );

    // `mitts clean` takes the store with the target.
    let clean = Command::new(env!("CARGO_BIN_EXE_mitts"))
        .arg("clean")
        .current_dir(p.root())
        .env("CARGO_HOME", p.dir.path().join("cargo-home"))
        .output()
        .unwrap();
    assert!(
        clean.status.success(),
        "{}",
        String::from_utf8_lossy(&clean.stderr)
    );
    assert!(!p.root().join("target/debug/.mitts-store").exists());

    // So does `rm -rf target`: nothing in the store is a directory its owner cannot write.
    assert!(p.mitts(&["build"], false) > 0);
    fs::remove_dir_all(p.root().join("target")).expect("rm -rf target");
}

#[test]
fn the_switch_shares_them_as_before() {
    let p = Project::new();
    assert!(p.mitts(&["build"], true) > 0);
    assert!(
        !files(&p.cache().join("cas/objects")).is_empty(),
        "shared on request"
    );
    assert!(local_objects(&p).is_empty());
}
