/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! `[env]` in `.cargo/config.toml`, with Cargo's rules: the variables reach rustc (`env!`),
//! the build script and the built binary; `force = true` overrides a variable the
//! environment already has, a plain entry does not; `relative = true` makes the value an
//! absolute path next to `.cargo`; and a changed value rebuilds what read it.

#![cfg(unix)]

use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

fn write_project(dir: &Path, greeting: &str) {
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::create_dir_all(dir.join(".cargo")).unwrap();
    fs::write(
        dir.join(".cargo/config.toml"),
        format!(
            "[env]\nGREETING = \"{greeting}\"\nFORCED = {{ value = \"from-config\", force = true }}\nASSETS = {{ value = \"assets\", relative = true }}\nBS_ONLY = \"seen-by-build-script\"\n"
        ),
    )
    .unwrap();
    fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\nbuild = \"build.rs\"\n",
    )
    .unwrap();
    fs::write(
        dir.join("build.rs"),
        "fn main() {\n    let assets = std::env::var(\"ASSETS\").unwrap();\n    assert!(std::path::Path::new(&assets).is_absolute(), \"relative = true gives an absolute path: {assets}\");\n    println!(\"cargo:rustc-env=BS_SAW={}\", std::env::var(\"BS_ONLY\").unwrap());\n    println!(\"cargo:rerun-if-changed=build.rs\");\n}\n",
    )
    .unwrap();
    fs::write(
        dir.join("src/main.rs"),
        "fn main() {\n    println!(\"{} {} {} {}\", env!(\"GREETING\"), env!(\"FORCED\"), env!(\"BS_SAW\"), std::env::var(\"GREETING\").unwrap_or_default());\n}\n",
    )
    .unwrap();
}

fn mitts(args: &[&str], dir: &Path, cache: &Path, env: &[(&str, &str)]) -> (bool, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mitts"));
    cmd.args(args)
        .arg("--manifest-path")
        .arg(dir.join("Cargo.toml"))
        .arg("--cache-dir")
        .arg(cache)
        .env("CI", "true")
        .env_remove("MITTS_CACHE_DIR")
        .env_remove("GREETING")
        .env_remove("FORCED");
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd.output().expect("run mitts");
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success(), text)
}

#[test]
fn config_env_reaches_rustc_build_scripts_and_run_with_force_and_relative_semantics() {
    let temp = TempDir::new().unwrap();
    let app = temp.path().join("app");
    write_project(&app, "hello");
    let cache = temp.path().join("cache");

    // `FORCED` is preset in the environment but `force = true` wins; `GREETING` is not
    // preset, so the config value applies.
    let (ok, text) = mitts(&["build"], &app, &cache, &[("FORCED", "from-env")]);
    assert!(ok, "build with [env]:\n{text}");
    let run = Command::new(app.join("target/mitts/debug/app"))
        .env_remove("GREETING")
        .output()
        .unwrap();
    // Run outside Mitts, the program itself sees no GREETING: the last field is empty.
    assert_eq!(
        String::from_utf8_lossy(&run.stdout).trim(),
        "hello from-config seen-by-build-script"
    );

    // A preset variable without `force` is left alone, as Cargo leaves it.
    let cache2 = temp.path().join("cache2");
    let (ok, text) = mitts(&["build"], &app, &cache2, &[("GREETING", "preset")]);
    assert!(ok, "{text}");
    let run = Command::new(app.join("target/mitts/debug/app"))
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&run.stdout).starts_with("preset from-config"),
        "{}",
        String::from_utf8_lossy(&run.stdout)
    );

    // `mitts run` passes the variables to the program itself.
    let (ok, text) = mitts(&["run"], &app, &cache, &[]);
    assert!(
        ok && text.contains("hello from-config seen-by-build-script hello"),
        "{text}"
    );

    // Changing the value recompiles the unit that reads it: the config is an input.
    write_project(&app, "changed");
    let (ok, text) = mitts(&["build"], &app, &cache, &[]);
    assert!(ok, "{text}");
    let run = Command::new(app.join("target/mitts/debug/app"))
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&run.stdout).starts_with("changed from-config"),
        "a changed [env] value is picked up: {}",
        String::from_utf8_lossy(&run.stdout)
    );
}
