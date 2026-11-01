/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! grpc-xds' build script writes `#[path = "<OUT_DIR>/generated/cel/mod.rs"] pub mod cel;`
//! with the absolute `OUT_DIR` into the generated module tree. Under Cargo the path is
//! permanent. Mitts ran the script in a staging directory and moved the output into the
//! store, so the next build that compiled the crate from the restored output (tonic: every
//! swarm worktree on the rig) failed with "couldn't read <staging>/…/cel/mod.rs". The saved
//! output now carries the store path, as the directives already did.

use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

fn write_project(dir: &Path) {
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\nbuild = \"build.rs\"\n",
    )
    .unwrap();
    fs::write(
        dir.join("build.rs"),
        r##"use std::{env, fs, path::PathBuf};
fn main() {
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let gen = out.join("generated");
    fs::create_dir_all(gen.join("inner")).unwrap();
    fs::write(gen.join("inner/mod.rs"), "pub fn answer() -> u32 { 42 }\n").unwrap();
    fs::write(
        gen.join("mod.rs"),
        format!("#[path = \"{}\"]\npub mod inner;\n", gen.join("inner/mod.rs").display()),
    )
    .unwrap();
    println!("cargo:rerun-if-changed=build.rs");
}
"##,
    )
    .unwrap();
    fs::write(
        dir.join("src/main.rs"),
        "mod generated {\n    include!(concat!(env!(\"OUT_DIR\"), \"/generated/mod.rs\"));\n}\n\
         fn main() { println!(\"{}\", generated::inner::answer()); }\n",
    )
    .unwrap();
}

fn mitts_build(dir: &Path, cache: &Path) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_mitts"))
        .arg("build")
        .arg("--manifest-path")
        .arg(dir.join("Cargo.toml"))
        .arg("--cache-dir")
        .arg(cache)
        .env("CI", "true")
        .env_remove("MITTS_CACHE_DIR")
        .output()
        .expect("run mitts");
    (
        out.status.success(),
        format!(
            "{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
    )
}

#[test]
fn generated_code_naming_the_absolute_out_dir_compiles_from_the_restored_build_script_output() {
    let temp = TempDir::new().unwrap();
    let app = temp.path().join("app");
    write_project(&app);
    let cache = temp.path().join("cache");

    let (ok, text) = mitts_build(&app, &cache);
    assert!(ok, "cold build:\n{text}");

    // The source changes, the build script does not: rustc compiles the crate again
    // against the build script output restored from the store.
    let main = app.join("src/main.rs");
    let mut src = fs::read_to_string(&main).unwrap();
    src.push_str("// touched\n");
    fs::write(&main, src).unwrap();
    let (ok, text) = mitts_build(&app, &cache);
    assert!(ok, "rebuild from the restored OUT_DIR:\n{text}");
    let run = Command::new(app.join("target/mitts/debug/app"))
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "42");

    // A second checkout sharing the store restores the same output.
    let other = temp.path().join("other");
    write_project(&other);
    let (ok, text) = mitts_build(&other, &cache);
    assert!(
        ok,
        "build of another checkout from the shared store:\n{text}"
    );
}
