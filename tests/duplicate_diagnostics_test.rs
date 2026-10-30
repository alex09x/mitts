/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A library used by a proc-macro and by a normal crate is compiled twice, for the host
//! and for the target, and warns twice. Cargo shows the warning once and counts the
//! repeat ("(1 duplicates)"); Mitts printed both copies, which the corpus parity check on
//! pest flagged.

use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

fn write_project(dir: &Path) {
    for sub in ["app/src", "shared/src", "pm/src"] {
        fs::create_dir_all(dir.join(sub)).unwrap();
    }
    fs::write(
        dir.join("Cargo.toml"),
        "[workspace]\nmembers = [\"app\", \"shared\", \"pm\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    fs::write(
        dir.join("app/Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
         [dependencies]\nshared = { path = \"../shared\" }\npm = { path = \"../pm\" }\n",
    )
    .unwrap();
    fs::write(dir.join("app/src/lib.rs"), "pub fn v() -> u64 { shared::val() + pm::one!() }\n").unwrap();
    fs::write(
        dir.join("shared/Cargo.toml"),
        "[package]\nname = \"shared\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(
        dir.join("shared/src/lib.rs"),
        "pub fn val() -> u64 { let unused_in_shared = 1; 7 }\n",
    )
    .unwrap();
    fs::write(
        dir.join("pm/Cargo.toml"),
        "[package]\nname = \"pm\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\nproc-macro = true\n\n[dependencies]\nshared = { path = \"../shared\" }\n",
    )
    .unwrap();
    fs::write(
        dir.join("pm/src/lib.rs"),
        "use proc_macro::TokenStream;\n#[proc_macro]\npub fn one(_: TokenStream) -> TokenStream { let _ = shared::val(); \"1u64\".parse().unwrap() }\n",
    )
    .unwrap();
}

#[test]
fn a_warning_from_a_crate_compiled_for_host_and_target_is_shown_once() {
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
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "check passes:\n{text}");
    assert_eq!(
        text.matches("warning: unused variable: `unused_in_shared`").count(),
        1,
        "the shared crate's warning is shown once:\n{text}"
    );
    assert!(text.contains("(1 duplicates)"), "the repeat is counted:\n{text}");
}
