/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! minijinja-cli lists `argument` and `argument-completions` both as dependencies and as
//! build-dependencies; `argument-completions` depends on `argument`. Mitts compiled the
//! binary against one instance of `argument` while `argument-completions` had been linked
//! against the other (host) instance, so rustc saw two `argument` crates and reported
//! `Cli<Arg>: CommandInfo` as unsatisfied although the impl exists (E0277). Every target
//! unit must link the target instance of a shared dependency, host units the host one.

use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

/// `app` is the only package of its manifest; `shim` and `helper` are path dependencies
/// outside any workspace, so, like registry crates, they get a target unit only when the
/// feature resolver activates them on the target side (a workspace member always has one).
fn write_project(dir: &Path) {
    for sub in ["app/src", "shim/src", "helper/src"] {
        fs::create_dir_all(dir.join(sub)).unwrap();
    }
    // `helper` is optional on both sides and enabled by a default feature, exactly like
    // minijinja-cli's `argument-completions` behind its `completions` feature.
    fs::write(
        dir.join("app/Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\nbuild = \"build.rs\"\n\n\
         [features]\ndefault = [\"completions\"]\ncompletions = [\"helper\"]\n\n\
         [dependencies]\nshim = { path = \"../shim\" }\nhelper = { path = \"../helper\", optional = true }\n\n\
         [build-dependencies]\nshim = { path = \"../shim\" }\nhelper = { path = \"../helper\", optional = true }\n",
    )
    .unwrap();
    fs::write(
        dir.join("app/build.rs"),
        "fn main() { helper::describe(&shim::Thing(1)); println!(\"cargo:rerun-if-changed=build.rs\"); }\n",
    )
    .unwrap();
    fs::write(
        dir.join("app/src/main.rs"),
        "fn main() { println!(\"{}\", helper::describe(&shim::Thing(2))); }\n",
    )
    .unwrap();
    fs::write(
        dir.join("shim/Cargo.toml"),
        "[package]\nname = \"shim\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(
        dir.join("shim/src/lib.rs"),
        "pub trait Info { fn name(&self) -> String; }\npub struct Thing(pub u32);\n\
         impl Info for Thing { fn name(&self) -> String { format!(\"thing{}\", self.0) } }\n",
    )
    .unwrap();
    fs::write(
        dir.join("helper/Cargo.toml"),
        "[package]\nname = \"helper\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
         [dependencies]\nshim = { path = \"../shim\" }\n",
    )
    .unwrap();
    fs::write(
        dir.join("helper/src/lib.rs"),
        "pub fn describe<T: shim::Info>(t: &T) -> String { t.name() }\n",
    )
    .unwrap();
}

#[test]
fn a_dependency_shared_by_build_and_target_links_one_instance_per_side() {
    let cache = TempDir::new().unwrap();
    let dir = TempDir::new().unwrap();
    write_project(dir.path());
    for args in [&["build"][..], &["test", "--no-run"][..]] {
        let out = Command::new(env!("CARGO_BIN_EXE_mitts"))
            .args(args)
            .arg("--manifest-path")
            .arg(dir.path().join("app/Cargo.toml"))
            .arg("--cache-dir")
            .arg(cache.path())
            .env("CI", "true")
            .env_remove("MITTS_CACHE_DIR")
            .output()
            .expect("run mitts");
        assert!(
            out.status.success(),
            "mitts {} links `shim` and `helper` consistently:\n{}\n{}",
            args[0],
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
