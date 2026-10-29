/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Cargo accepts `proc_macro`, `crate_type` and `required_features` with underscores as
//! well as the hyphenated keys. derive_arbitrary 1.3.2 (a tokei dependency) publishes
//! `[lib] proc_macro = true`; Mitts read only `proc-macro`, planned the crate as a plain
//! library and rustc rejected `#[proc_macro_derive]` outside a proc-macro crate.

use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

fn write_project(dir: &Path) {
    for sub in ["app/src", "pm/src"] {
        fs::create_dir_all(dir.join(sub)).unwrap();
    }
    fs::write(
        dir.join("Cargo.toml"),
        "[workspace]\nmembers = [\"app\", \"pm\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    fs::write(
        dir.join("app/Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
         [dependencies]\npm = { path = \"../pm\" }\n",
    )
    .unwrap();
    fs::write(
        dir.join("app/src/lib.rs"),
        "#[derive(pm::Answer)]\npub struct Q;\npub fn v() -> u32 { Q::answer() }\n",
    )
    .unwrap();
    fs::write(
        dir.join("pm/Cargo.toml"),
        "[package]\nname = \"pm\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\nproc_macro = true\n",
    )
    .unwrap();
    fs::write(
        dir.join("pm/src/lib.rs"),
        "use proc_macro::TokenStream;\n\
         #[proc_macro_derive(Answer)]\npub fn answer(input: TokenStream) -> TokenStream {\n\
         \x20   let name = input.into_iter().find_map(|t| match t { proc_macro::TokenTree::Ident(i) if i.to_string() != \"pub\" && i.to_string() != \"struct\" => Some(i.to_string()), _ => None }).unwrap();\n\
         \x20   format!(\"impl {name} {{ pub fn answer() -> u32 {{ 42 }} }}\").parse().unwrap()\n}\n",
    )
    .unwrap();
}

#[test]
fn underscore_proc_macro_key_plans_a_proc_macro_crate() {
    let cache = TempDir::new().unwrap();
    let dir = TempDir::new().unwrap();
    write_project(dir.path());
    let out = Command::new(env!("CARGO_BIN_EXE_mitts"))
        .args(["check", "--workspace", "--manifest-path"])
        .arg(dir.path().join("Cargo.toml"))
        .arg("--cache-dir")
        .arg(cache.path())
        .env("CI", "true")
        .env_remove("MITTS_CACHE_DIR")
        .output()
        .expect("run mitts");
    assert!(
        out.status.success(),
        "`proc_macro = true` makes a proc-macro crate:\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}
