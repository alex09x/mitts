/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Building for a target other than the host, as Cargo does with `--target`: the package
//! and its normal dependencies are compiled for the target into `target/<triple>/`, build
//! scripts, build-dependencies and proc macros for the host; `[target.'cfg(..)']`
//! dependencies follow the target; a build script sees the target's `CARGO_CFG_*`; the
//! executable has the target's name (`app.wasm`). The target here is
//! `wasm32-unknown-unknown`, linked by the toolchain's rust-lld.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use tempfile::TempDir;

const TARGET: &str = "wasm32-unknown-unknown";

fn write(path: impl AsRef<Path>, contents: &str) {
    let path = path.as_ref();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// Whether the toolchain has the target's standard library. CI installs it; elsewhere the
/// test says it is skipped instead of failing on a missing component.
fn target_installed() -> bool {
    let sysroot = Command::new("rustc")
        .args(["--print", "sysroot"])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    Path::new(&sysroot)
        .join("lib/rustlib")
        .join(TARGET)
        .join("lib")
        .is_dir()
}

fn project(root: &Path) {
    write(
        root.join("Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
         [dependencies]\ndep = { path = \"dep\" }\npm = { path = \"pm\" }\n\n\
         [target.'cfg(target_arch = \"wasm32\")'.dependencies]\nwasm_only = { path = \"wasm_only\" }\n\n\
         [target.'cfg(not(target_arch = \"wasm32\"))'.dependencies]\nnative_only = { path = \"native_only\" }\n\n\
         [build-dependencies]\nbdep = { path = \"bdep\" }\n",
    );
    write(
        root.join("build.rs"),
        "fn main() {\n\
         \x20   // Runs on the host, configured for the target.\n\
         \x20   let arch = std::env::var(\"CARGO_CFG_TARGET_ARCH\").unwrap();\n\
         \x20   let target = std::env::var(\"TARGET\").unwrap();\n\
         \x20   println!(\"cargo:rustc-env=BUILT_FOR={arch}/{target}/{}\", bdep::host_value());\n\
         }\n",
    );
    write(
        root.join("src/lib.rs"),
        "pub const BUILT_FOR: &str = env!(\"BUILT_FOR\");\n\
         #[pm::doubled]\npub fn base() -> u32 { dep::value() }\n\
         #[cfg(target_arch = \"wasm32\")]\npub fn platform() -> u32 { wasm_only::value() }\n\
         #[cfg(not(target_arch = \"wasm32\"))]\npub fn platform() -> u32 { native_only::value() }\n",
    );
    write(
        root.join("src/main.rs"),
        "fn main() { let _ = (app::base(), app::platform(), app::BUILT_FOR); }\n",
    );
    for (name, body) in [
        ("dep", "pub fn value() -> u32 { 21 }\n"),
        ("wasm_only", "pub fn value() -> u32 { 32 }\n"),
        ("native_only", "pub fn value() -> u32 { 64 }\n"),
        ("bdep", "pub fn host_value() -> &'static str { \"host\" }\n"),
    ] {
        write(
            root.join(name).join("Cargo.toml"),
            &format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
        );
        write(root.join(name).join("src/lib.rs"), body);
    }
    write(
        root.join("pm/Cargo.toml"),
        "[package]\nname = \"pm\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[lib]\nproc-macro = true\n",
    );
    write(
        root.join("pm/src/lib.rs"),
        "extern crate proc_macro;\nuse proc_macro::TokenStream;\n\
         /// Keeps the function as it is (only proves the macro runs on the host).\n\
         #[proc_macro_attribute]\n\
         pub fn doubled(_: TokenStream, item: TokenStream) -> TokenStream { item }\n",
    );
}

fn mitts(root: &Path, args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mitts"));
    cmd.args(args)
        .arg("--cache-dir")
        .arg(root.join("cache"))
        .current_dir(root)
        .env("CARGO_HOME", root.join("cargo-home"))
        .env_remove("MITTS_CACHE_DIR");
    for name in [
        "CARGO_TARGET_DIR",
        "CARGO_BUILD_TARGET",
        "CARGO_ENCODED_RUSTFLAGS",
        "RUSTFLAGS",
        "CARGO_BUILD_RUSTFLAGS",
        "RUSTC_WRAPPER",
        "RUSTC_WORKSPACE_WRAPPER",
        "CARGO_BUILD_RUSTC_WRAPPER",
    ] {
        cmd.env_remove(name);
    }
    cmd.output().expect("run Mitts")
}

fn crate_lines<'a>(log: &'a str, name: &str) -> Vec<&'a str> {
    log.lines()
        .filter(|l| l.contains(&format!("--crate-name {name} ")))
        .collect()
}

#[test]
fn a_foreign_target_builds_with_host_tools_and_target_dependencies() {
    if !target_installed() {
        assert!(
            std::env::var_os("CI").is_none(),
            "CI must install the {TARGET} standard library"
        );
        eprintln!("skipped: rustup target add {TARGET}");
        return;
    }
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("proj");
    project(&root);

    let out = mitts(&root, &["build", "-vv", "--target", TARGET]);
    let log = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(out.status.success(), "{log}");

    // Target units get --target; host units (build script, build-dependency, proc macro)
    // do not.
    for name in ["app", "dep", "wasm_only"] {
        let lines = crate_lines(&log, name);
        assert!(!lines.is_empty(), "{name} built: {log}");
        assert!(
            lines
                .iter()
                .all(|l| l.contains(&format!("--target {TARGET}"))),
            "{name} for the target: {lines:?}"
        );
    }
    for name in ["build_script_build", "bdep", "pm"] {
        let lines = crate_lines(&log, name);
        assert!(!lines.is_empty(), "{name} built: {log}");
        assert!(
            lines.iter().all(|l| !l.contains("--target wasm32")),
            "{name} for the host: {lines:?}"
        );
    }
    assert!(
        crate_lines(&log, "native_only").is_empty(),
        "a cfg(not(wasm32)) dependency is not in a wasm32 build: {log}"
    );

    let out_dir = root.join("target").join(TARGET).join("debug");
    assert!(
        out_dir.join("app.wasm").is_file(),
        "the executable is app.wasm"
    );
    assert!(out_dir.join("libapp.rlib").is_file());
    assert!(
        !root.join("target/debug/app").exists() && !root.join("target/debug/libapp.rlib").exists(),
        "nothing for the target lands in the host directory"
    );
    let wasm = fs::read(out_dir.join("app.wasm")).unwrap();
    assert_eq!(&wasm[..4], b"\0asm", "a WebAssembly module");

    // The same build again is a full cache hit.
    let again = mitts(&root, &["build", "-v", "--target", TARGET]);
    let log = String::from_utf8_lossy(&again.stderr).to_string();
    assert!(
        again.status.success() && log.contains("compiled=0 "),
        "nothing is compiled again: {log}"
    );

    // A host build does not reuse the wasm32 units, and takes the native dependency.
    let host = mitts(&root, &["build", "-vv"]);
    let log = String::from_utf8_lossy(&host.stderr).to_string();
    assert!(host.status.success(), "{log}");
    assert!(!crate_lines(&log, "native_only").is_empty(), "{log}");
    assert!(crate_lines(&log, "wasm_only").is_empty(), "{log}");
    assert!(root
        .join("target/debug")
        .join(format!("app{}", std::env::consts::EXE_SUFFIX))
        .is_file());
}
