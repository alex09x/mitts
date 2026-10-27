/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A dependency's build script may point rustc at a native library it built or found
//! (`cargo:rustc-link-search`). A library linked as `dylib` is resolved only when the
//! final executable is linked, by a unit that never ran that build script, so Cargo passes
//! every dependency's search directories to its dependents. actix-web's tests on macOS
//! stopped at `ld: library 'ssl' not found` without that.

use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

/// `app` (tests) → `mid` (plain lib) → `nat` (lib with a build script that compiles
/// `libforty` as a shared library in its OUT_DIR and links it as `dylib`). The plain
/// crate in the middle is the one that must pass the directory on, and it must do so
/// whether it was compiled or restored from the cache.
fn write_project(dir: &Path) {
    for sub in ["src", "tests", "mid/src", "nat/src"] {
        fs::create_dir_all(dir.join(sub)).unwrap();
    }
    fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
         [dependencies]\nmid = { path = \"mid\" }\n\n[workspace]\nmembers = [\"mid\", \"nat\"]\n",
    )
    .unwrap();
    fs::write(dir.join("src/lib.rs"), "pub fn answer() -> i32 { mid::value() }\n").unwrap();
    fs::write(
        dir.join("tests/link.rs"),
        "#[test]\nfn links_the_native_library_of_a_dependency() { assert_eq!(app::answer(), 42); }\n",
    )
    .unwrap();
    fs::write(
        dir.join("mid/Cargo.toml"),
        "[package]\nname = \"mid\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
         [dependencies]\nnat = { path = \"../nat\" }\n",
    )
    .unwrap();
    fs::write(dir.join("mid/src/lib.rs"), "pub fn value() -> i32 { unsafe { nat::forty_two() } }\n").unwrap();
    fs::write(
        dir.join("nat/Cargo.toml"),
        "[package]\nname = \"nat\"\nversion = \"0.1.0\"\nedition = \"2021\"\nbuild = \"build.rs\"\n",
    )
    .unwrap();
    fs::write(
        dir.join("nat/src/lib.rs"),
        "extern \"C\" {\n    pub fn forty_two() -> i32;\n}\n",
    )
    .unwrap();
    fs::write(
        dir.join("nat/build.rs"),
        r#"use std::path::PathBuf;
use std::process::Command;
fn main() {
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let src = out.join("forty.c");
    std::fs::write(&src, "int forty_two(void) { return 42; }\n").unwrap();
    let ext = if cfg!(target_os = "macos") { "dylib" } else { "so" };
    let lib = out.join(format!("libforty.{ext}"));
    let status = Command::new("cc")
        .args(["-shared", "-fPIC", "-o"])
        .arg(&lib)
        .arg(&src)
        .status()
        .expect("cc");
    assert!(status.success(), "cc failed");
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=dylib=forty");
    println!("cargo:rerun-if-changed=build.rs");
}
"#,
    )
    .unwrap();
}

#[test]
fn a_test_binary_links_a_dylib_found_through_a_dependency_build_script() {
    if Command::new("cc").arg("--version").output().is_err() {
        eprintln!("no C compiler on this host; skipping");
        return;
    }
    let cache = TempDir::new().unwrap();
    let dir = TempDir::new().unwrap();
    write_project(dir.path());

    let run = || {
        let out = Command::new(env!("CARGO_BIN_EXE_mitts"))
            .args(["test", "--no-run", "--manifest-path"])
            .arg(dir.path().join("Cargo.toml"))
            .arg("--cache-dir")
            .arg(cache.path())
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
    };
    let (ok, text) = run();
    assert!(ok, "the test executable links libforty through nat's search directory:\n{text}");

    // Warm: `nat` and `mid` come back from the cache, only the edited test is compiled
    // and linked. The restored `mid` must still hand `nat`'s directory on.
    fs::write(
        dir.path().join("tests/link.rs"),
        "#[test]\nfn links_after_an_edit() { assert_eq!(app::answer() + 1, 43); }\n",
    )
    .unwrap();
    let (ok, text) = run();
    assert!(ok, "a test linked against cached dependencies finds the library too:\n{text}");
}
