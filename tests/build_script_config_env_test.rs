/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! `[env]` of the project's `.cargo/config.toml` reaches the build script of a dependency
//! that lives outside the project, and a changed value runs it again, as under Cargo.
//! hitmeup's `[env] LIBSQLITE3_FLAGS` never reached libsqlite3-sys: its directory, in the
//! shared source store, has none of the project's configs, and the cached run was checked
//! against Mitts' own environment, where the variable is not set.

use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

const VAR: &str = "MITTS_TEST_DEP_FLAGS";

fn project() -> TempDir {
    let dir = TempDir::new().unwrap();
    let ws = dir.path().join("ws");
    let dep = dir.path().join("elsewhere/flagged");
    fs::create_dir_all(ws.join("src")).unwrap();
    fs::create_dir_all(ws.join(".cargo")).unwrap();
    fs::create_dir_all(dep.join("src")).unwrap();
    fs::write(
        dep.join("Cargo.toml"),
        "[package]\nname = \"flagged\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(
        dep.join("build.rs"),
        format!(
            "fn main() {{\n    println!(\"cargo:rerun-if-env-changed={VAR}\");\n    \
             let v = std::env::var(\"{VAR}\").unwrap_or_else(|_| \"unset\".into());\n    \
             let out = std::env::var(\"OUT_DIR\").unwrap();\n    \
             std::fs::write(format!(\"{{out}}/flags.rs\"), format!(\"pub const FLAGS: &str = {{v:?}};\")).unwrap();\n}}\n"
        ),
    )
    .unwrap();
    fs::write(
        dep.join("src/lib.rs"),
        "include!(concat!(env!(\"OUT_DIR\"), \"/flags.rs\"));\n",
    )
    .unwrap();
    fs::write(
        ws.join("Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
         [dependencies]\nflagged = { path = \"../elsewhere/flagged\" }\n",
    )
    .unwrap();
    fs::write(
        ws.join("src/main.rs"),
        "fn main() { println!(\"flags={}\", flagged::FLAGS); }\n",
    )
    .unwrap();
    dir
}

fn set_flags(dir: &Path, value: &str) {
    fs::write(
        dir.join("ws/.cargo/config.toml"),
        format!("[env]\n{VAR} = \"{value}\"\n"),
    )
    .unwrap();
}

fn run(dir: &Path) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_mitts"))
        .args(["run", "-q", "--cache-dir"])
        .arg(dir.join("cache"))
        .current_dir(dir.join("ws"))
        .env("CARGO_HOME", dir.join("cargo-home"))
        .env_remove(VAR)
        .env_remove("MITTS_CACHE_DIR")
        .env_remove("RUSTC_WRAPPER")
        .env_remove("CARGO_BUILD_RUSTC_WRAPPER")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn config_env_reaches_a_dependencys_build_script_and_a_change_reruns_it() {
    let dir = project();
    set_flags(dir.path(), "-DONE");
    assert_eq!(run(dir.path()), "flags=-DONE", "the script sees [env]");
    set_flags(dir.path(), "-DTWO");
    assert_eq!(
        run(dir.path()),
        "flags=-DTWO",
        "a changed [env] value reruns it"
    );
    set_flags(dir.path(), "-DONE");
    assert_eq!(
        run(dir.path()),
        "flags=-DONE",
        "and back, from the cache or a rerun"
    );
}
