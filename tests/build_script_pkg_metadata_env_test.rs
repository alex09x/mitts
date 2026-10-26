/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A build script is compiled and run with the same `CARGO_PKG_*` metadata Cargo gives
//! it (lsd's build.rs derives clap's `about` from `CARGO_PKG_DESCRIPTION` at compile time).

use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

fn write_package(dir: &Path) {
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname = \"meta_probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\ndescription = \"probe description\"\nrepository = \"https://example.invalid/meta\"\nlicense = \"MIT\"\nauthors = [\"A <a@example.invalid>\", \"B\"]\n",
    )
    .unwrap();
    // Compile-time reads in the build script itself, then runtime reads when it runs.
    fs::write(
        dir.join("build.rs"),
        r#"const DESC: &str = env!("CARGO_PKG_DESCRIPTION");
const REPO: &str = env!("CARGO_PKG_REPOSITORY");
fn main() {
    assert_eq!(DESC, "probe description");
    assert_eq!(REPO, "https://example.invalid/meta");
    assert_eq!(std::env::var("CARGO_PKG_LICENSE").unwrap(), "MIT");
    assert_eq!(std::env::var("CARGO_PKG_AUTHORS").unwrap(), "A <a@example.invalid>:B");
    println!("cargo:rustc-env=PROBE_DESC={}", std::env::var("CARGO_PKG_DESCRIPTION").unwrap());
}
"#,
    )
    .unwrap();
    fs::write(
        dir.join("src/lib.rs"),
        "pub const FROM_SCRIPT: &str = env!(\"PROBE_DESC\");\n",
    )
    .unwrap();
}

#[test]
fn build_scripts_see_the_package_metadata_when_compiled_and_run() {
    let cache = TempDir::new().unwrap();
    let temp = TempDir::new().unwrap();
    let dir = temp.path().join("meta_probe");
    write_package(&dir);
    let out = Command::new(env!("CARGO_BIN_EXE_mitts"))
        .args(["build", "--manifest-path"])
        .arg(dir.join("Cargo.toml"))
        .arg("--cache-dir")
        .arg(cache.path())
        .env("CI", "true")
        .env_remove("MITTS_CACHE_DIR")
        .output()
        .expect("run mitts");
    assert!(
        out.status.success(),
        "mitts build failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
