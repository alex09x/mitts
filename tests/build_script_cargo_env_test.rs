/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A build script gets the environment Cargo documents for it ("Environment variables Cargo
//! sets for build scripts"). av-scenechange 0.14 unwraps `CARGO_ENCODED_RUSTFLAGS`, which
//! Cargo sets even when empty, and `built` 0.8 requires `RUSTDOC`; both failed under Mitts.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use tempfile::TempDir;

/// Variables Cargo always gives a build script.
const ALWAYS: &[&str] = &[
    "CARGO",
    "CARGO_MANIFEST_DIR",
    "CARGO_MANIFEST_PATH",
    "CARGO_PKG_NAME",
    "CARGO_PKG_VERSION",
    "CARGO_PKG_VERSION_MAJOR",
    "CARGO_PKG_VERSION_MINOR",
    "CARGO_PKG_VERSION_PATCH",
    "CARGO_PKG_VERSION_PRE",
    "CARGO_PKG_AUTHORS",
    "CARGO_PKG_DESCRIPTION",
    "CARGO_PKG_HOMEPAGE",
    "CARGO_PKG_REPOSITORY",
    "CARGO_PKG_LICENSE",
    "CARGO_PKG_README",
    "CARGO_PKG_RUST_VERSION",
    "CARGO_ENCODED_RUSTFLAGS",
    "CARGO_CFG_TARGET_OS",
    "CARGO_CFG_TARGET_ARCH",
    "CARGO_CFG_TARGET_POINTER_WIDTH",
    "OUT_DIR",
    "TARGET",
    "HOST",
    "NUM_JOBS",
    "OPT_LEVEL",
    "DEBUG",
    "PROFILE",
    "RUSTC",
    "RUSTDOC",
];

fn write_file(path: impl AsRef<Path>, contents: &str) {
    let path = path.as_ref();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// A binary whose build script fails on any missing variable and passes a few values on.
fn project(config: Option<&str>) -> TempDir {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    write_file(
        root.join("Cargo.toml"),
        "[package]\nname = \"envprobe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    let names = ALWAYS
        .iter()
        .map(|n| format!("\"{n}\""))
        .collect::<Vec<_>>()
        .join(", ");
    write_file(
        root.join("build.rs"),
        &format!(
            "fn main() {{\n\
             let missing: Vec<&str> = [{names}].into_iter()\n\
                 .filter(|n| std::env::var_os(n).is_none()).collect();\n\
             assert!(missing.is_empty(), \"missing build-script env: {{:?}}\", missing);\n\
             let flags = std::env::var(\"CARGO_ENCODED_RUSTFLAGS\").unwrap().replace('\\x1f', \" \");\n\
             println!(\"cargo:rustc-env=SEEN_FLAGS=[{{flags}}]\");\n\
             let probe = std::env::var_os(\"CARGO_CFG_MITTS_PROBE\").is_some();\n\
             println!(\"cargo:rustc-env=SEEN_CFG_PROBE={{probe}}\");\n\
             let linker = std::env::var(\"RUSTC_LINKER\").unwrap_or_default();\n\
             println!(\"cargo:rustc-env=SEEN_LINKER={{linker}}\");\n\
             let host = std::env::var(\"HOST\").unwrap() == std::env::var(\"TARGET\").unwrap();\n\
             println!(\"cargo:rustc-env=SEEN_HOST_IS_TARGET={{host}}\");\n\
             let rustdoc = std::path::Path::new(&std::env::var(\"RUSTDOC\").unwrap())\n\
                 .file_name().unwrap().to_string_lossy().starts_with(\"rustdoc\");\n\
             println!(\"cargo:rustc-env=SEEN_RUSTDOC={{rustdoc}}\");\n}}\n"
        ),
    );
    write_file(
        root.join("src/main.rs"),
        "fn main() {\n\
         println!(\"flags={} probe={} linker={} host_is_target={} rustdoc={}\", \
         env!(\"SEEN_FLAGS\"), env!(\"SEEN_CFG_PROBE\"), env!(\"SEEN_LINKER\"), \
         env!(\"SEEN_HOST_IS_TARGET\"), env!(\"SEEN_RUSTDOC\"));\n}\n",
    );
    if let Some(config) = config {
        write_file(root.join(".cargo/config.toml"), config);
    }
    temp
}

fn mitts(root: &Path, args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mitts"));
    cmd.args(args)
        .arg("--cache-dir")
        .arg(root.join("mitts-cache"))
        .current_dir(root)
        .env_remove("MITTS_CACHE_DIR");
    for name in [
        "CARGO_TARGET_DIR",
        "CARGO_BUILD_TARGET_DIR",
        "CARGO_BUILD_TARGET",
        "CARGO_ENCODED_RUSTFLAGS",
        "RUSTFLAGS",
        "CARGO_BUILD_RUSTFLAGS",
        "RUSTDOC",
        "RUSTC_WRAPPER",
        "RUSTC_WORKSPACE_WRAPPER",
        "CARGO_BUILD_RUSTC_WRAPPER",
    ] {
        cmd.env_remove(name);
    }
    cmd.output().expect("run Mitts")
}

fn build_and_run(root: &Path) -> String {
    let out = mitts(root, &["build", "-q"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = Command::new(root.join("target/debug/envprobe"))
        .output()
        .unwrap();
    assert!(run.status.success());
    String::from_utf8_lossy(&run.stdout).trim().to_string()
}

#[test]
fn a_build_script_gets_every_variable_cargo_always_sets() {
    let temp = project(None);
    assert_eq!(
        build_and_run(temp.path()),
        "flags=[] probe=false linker= host_is_target=true rustdoc=true"
    );
}

#[test]
fn rustflags_reach_the_script_encoded_and_in_its_cfg() {
    let temp = project(Some("[build]\nrustflags = [\"--cfg\", \"mitts_probe\"]\n"));
    assert_eq!(
        build_and_run(temp.path()),
        "flags=[--cfg mitts_probe] probe=true linker= host_is_target=true rustdoc=true"
    );
}

#[test]
fn a_configured_linker_reaches_the_script_as_rustc_linker() {
    let temp = project(Some("[target.'cfg(unix)']\nlinker = \"cc\"\n"));
    assert_eq!(
        build_and_run(temp.path()),
        "flags=[] probe=false linker=cc host_is_target=true rustdoc=true"
    );
}

/// Changed rustflags change what the script sees, so its cached run is not reused.
#[test]
fn changed_rustflags_run_the_script_again() {
    let temp = project(Some("[build]\nrustflags = [\"--cfg\", \"first\"]\n"));
    let root = temp.path();
    assert!(build_and_run(root).starts_with("flags=[--cfg first]"));
    write_file(
        root.join(".cargo/config.toml"),
        "[build]\nrustflags = [\"--cfg\", \"second\"]\n",
    );
    assert!(build_and_run(root).starts_with("flags=[--cfg second]"));
}

/// An overridden rustdoc counts by its version: a script that reads `$RUSTDOC -V` runs again
/// when the override changes, while the rustdoc path itself stays out of the key.
#[cfg(unix)]
#[test]
fn a_changed_rustdoc_override_runs_the_script_again() {
    use std::os::unix::fs::PermissionsExt;
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    write_file(
        root.join("Cargo.toml"),
        "[package]\nname = \"docprobe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write_file(
        root.join("build.rs"),
        "fn main() {\n\
         let out = std::process::Command::new(std::env::var(\"RUSTDOC\").unwrap())\n\
             .arg(\"-V\").output().unwrap();\n\
         println!(\"cargo:rustc-env=SEEN_DOC={}\", String::from_utf8_lossy(&out.stdout).trim());\n}\n",
    );
    write_file(
        root.join("src/main.rs"),
        "fn main() { println!(\"{}\", env!(\"SEEN_DOC\")); }\n",
    );
    let fake = root.join("fake-rustdoc");
    let run_with = |version: &str| {
        write_file(&fake, &format!("#!/bin/sh\necho \"rustdoc {version}\"\n"));
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mitts"));
        cmd.args(["build", "-q", "--cache-dir"])
            .arg(root.join("mitts-cache"))
            .current_dir(root)
            .env_remove("MITTS_CACHE_DIR")
            .env_remove("CARGO_TARGET_DIR")
            .env_remove("CARGO_ENCODED_RUSTFLAGS")
            .env_remove("RUSTFLAGS")
            .env("RUSTDOC", &fake);
        let out = cmd.output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let run = Command::new(root.join("target/debug/docprobe"))
            .output()
            .unwrap();
        String::from_utf8_lossy(&run.stdout).trim().to_string()
    };
    assert_eq!(run_with("1.0.0"), "rustdoc 1.0.0");
    assert_eq!(run_with("2.0.0"), "rustdoc 2.0.0");
}
