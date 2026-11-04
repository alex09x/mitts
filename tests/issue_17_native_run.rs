/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use clap::Parser;
use mitts::adapter::rust::dag::CompilationDag;
use mitts::adapter::rust::dag_unit_externs::resolve_extern_crates_with_target;
use mitts::adapter::rust::pipelining::{classify_dep_requirement, DepRequirement};
use mitts::adapter::rust::{EffectivePlanExplanation, PlannedCompilationUnit};
use mitts::cli::{Cli, Commands};
use mitts::core::action::ToolchainInfo;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::tempdir;

fn cli_features(args: &[&str]) -> Vec<String> {
    let cli = Cli::try_parse_from(args).expect("parse Mitts CLI args");
    match cli.command {
        Commands::Build(args) | Commands::Rustc(args) => args.features,
        Commands::Check(args) => args.features,
        Commands::Clippy(args) => args.features,
        Commands::Test(args) => args.features,
        Commands::Coverage(args) | Commands::LlvmCov(args) => args.features,
        Commands::Run(args) => args.build_args.features,
        Commands::Explain(args) => args.features,
        Commands::Metadata(args) => args.features,
        other => panic!("unexpected command: {other:?}"),
    }
}

fn unit(package_id: &str, kind: &str, dependencies: Vec<String>) -> PlannedCompilationUnit {
    let package_name = package_id.split('@').next().unwrap_or(package_id);
    PlannedCompilationUnit {
        package_id: package_id.to_string(),
        package_name: package_name.to_string(),
        version: "0.1.0".to_string(),
        target_name: package_name.replace('-', "_"),
        kind: kind.to_string(),
        dependencies,
        ..PlannedCompilationUnit::default()
    }
}

fn write_file(root: &Path, relative: &str, body: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

#[test]
fn feature_lists_accept_comma_and_space_delimiters_across_cli_commands() {
    let commands = [
        "build", "rustc", "check", "clippy", "test", "coverage", "llvm-cov", "run", "explain",
        "metadata",
    ];
    for command in commands {
        let parsed = cli_features(&["mitts", command, "--features", "uniffi-cli,ssh"]);
        assert_eq!(parsed, ["uniffi-cli", "ssh"], "command={command}");
    }

    let parsed = cli_features(&[
        "mitts",
        "run",
        "--features",
        "uniffi-cli",
        "ssh",
        "--bin",
        "uniffi-bindgen",
    ]);
    assert_eq!(parsed, ["uniffi-cli", "ssh"]);
}

#[test]
fn multi_crate_type_library_consumers_require_full_transitive_artifacts() {
    let leaf = unit("leaf@0.1.0", "lib", vec![]);
    let middle = unit("middle@0.1.0", "lib", vec![leaf.unit_id()]);
    let root = unit(
        "root@0.1.0",
        "staticlib,cdylib,rlib",
        vec![middle.unit_id()],
    );

    assert_eq!(
        classify_dep_requirement(&root, &middle),
        DepRequirement::FullArtifact
    );

    let root_id = root.unit_id();
    let leaf_id = leaf.unit_id();
    let explanation = EffectivePlanExplanation {
        manifest_path: PathBuf::new(),
        target_directory: PathBuf::new(),
        profile: "dev".to_string(),
        toolchain: ToolchainInfo {
            name: "rustc".to_string(),
            version: "1.97.1".to_string(),
            target_triple: None,
        },
        units: vec![root, middle, leaf],
        manifest_snapshot: Default::default(),
    };
    let dag = CompilationDag::from_explanation(&explanation).unwrap();
    let root_node = dag.get_node(&root_id).unwrap();
    assert!(root_node.dependencies.contains(&leaf_id));
}

#[test]
fn multi_crate_type_consumers_use_rlib_even_when_rmeta_exists() {
    let temp = tempdir().unwrap();
    let manifest = temp.path().join("Cargo.toml");
    write_file(
        temp.path(),
        "Cargo.toml",
        "[package]\nname = \"consumer\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    let source = temp.path().join("src/lib.rs");
    write_file(temp.path(), "src/lib.rs", "pub fn value() {}\n");
    let rlib = temp.path().join("artifacts/libdep.rlib");
    fs::create_dir_all(rlib.parent().unwrap()).unwrap();
    fs::write(&rlib, b"rlib").unwrap();
    fs::write(rlib.with_extension("rmeta"), b"metadata").unwrap();
    let unit = PlannedCompilationUnit {
        package_id: "consumer@0.1.0".to_string(),
        package_name: "consumer".to_string(),
        version: "0.1.0".to_string(),
        target_name: "consumer".to_string(),
        kind: "staticlib,cdylib,rlib".to_string(),
        src_path: source,
        manifest_path: manifest,
        dependencies: vec!["dep@1.0.0".to_string()],
        ..PlannedCompilationUnit::default()
    };
    let completed = [("dep@1.0.0".to_string(), rlib.clone())]
        .into_iter()
        .collect();

    let externs = resolve_extern_crates_with_target(&unit, &completed, false);

    assert_eq!(externs, [("dep".to_string(), rlib)]);
}

#[test]
fn run_links_multi_crate_type_library_and_preserves_requested_features() {
    let temp = tempdir().unwrap();
    let root = temp.path();
    let target = root.join("target");
    let cache = root.join("cache");

    write_file(
        root,
        "Cargo.toml",
        r#"[package]
name = "issue17-native"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["staticlib", "cdylib", "rlib"]

[[bin]]
name = "uniffi-bindgen"
path = "src/main.rs"
required-features = ["uniffi-cli", "ssh"]

[features]
default = []
uniffi-cli = []
ssh = ["dep:bridge_dep"]

[dependencies]
bridge_dep = { package = "bridge-dep", path = "bridge", optional = true }
"#,
    );
    write_file(
        root,
        "src/lib.rs",
        "#[cfg(feature = \"ssh\")]\npub fn answer() -> u8 { bridge_dep::answer() }\n#[cfg(not(feature = \"ssh\"))]\npub fn answer() -> u8 { 0 }\n",
    );
    write_file(
        root,
        "src/main.rs",
        "fn main() { println!(\"consumer-value={}\", issue17_native::answer()); }\n",
    );
    write_file(
        root,
        "bridge/Cargo.toml",
        "[package]\nname = \"bridge-dep\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nleaf_dep = { package = \"leaf-dep\", path = \"../leaf\" }\n",
    );
    write_file(
        root,
        "bridge/src/lib.rs",
        "pub fn answer() -> u8 { leaf_dep::answer() }\n",
    );
    write_file(
        root,
        "leaf/Cargo.toml",
        "[package]\nname = \"leaf-dep\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write_file(root, "leaf/src/lib.rs", "pub fn answer() -> u8 { 42 }\n");

    let run = || {
        Command::new(env!("CARGO_BIN_EXE_mitts"))
            .current_dir(root)
            .args([
                "run",
                "--quiet",
                "--bin",
                "uniffi-bindgen",
                "--features",
                "uniffi-cli,ssh",
                "--manifest-path",
            ])
            .arg(root.join("Cargo.toml"))
            .arg("--target-dir")
            .arg(&target)
            .arg("--cache-dir")
            .arg(&cache)
            .output()
            .unwrap()
    };
    let output = run();

    assert!(
        output.status.success(),
        "mitts run failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "consumer-value=42"
    );

    let warm_output = run();
    assert!(
        warm_output.status.success(),
        "warm mitts run failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&warm_output.stdout),
        String::from_utf8_lossy(&warm_output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&warm_output.stdout).trim(),
        "consumer-value=42"
    );

    let debug = target.join("debug");
    assert!(debug.join("deps/libissue17_native.rlib").is_file());
    assert!(debug.join("libissue17_native.a").is_file());
    let dylib_ext = if cfg!(target_os = "macos") {
        "dylib"
    } else {
        "so"
    };
    assert!(debug
        .join(format!("deps/libissue17_native.{dylib_ext}"))
        .is_file());
}
