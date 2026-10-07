/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::{RustAdapter, RustBuildOptions};
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn fixture_manifest() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("hello-rust")
        .join("Cargo.toml")
}

#[test]
fn test_fixture_uncached_build_and_oracle() {
    let manifest = fixture_manifest();
    assert!(
        manifest.exists(),
        "Fixture manifest must exist: {:?}",
        manifest
    );

    let opt = RustBuildOptions {
        manifest_path: Some(manifest.clone()),
        ..Default::default()
    };

    let (plan, result) = RustAdapter::build(&opt).expect("Uncached build must succeed");
    assert!(result.success, "Build should indicate success");
    assert_eq!(result.exit_code, 0, "Build exit code must be 0");
    assert!(!plan.actions.is_empty(), "Plan must have recorded actions");

    // Verify isolated .mitts/target directory was used
    let target_dir = manifest.parent().unwrap().join(".mitts").join("target");
    assert_eq!(plan.target_directory, target_dir);
    assert!(
        target_dir.exists(),
        "Target directory must exist: {:?}",
        target_dir
    );

    // Locate the built binary in the isolated target
    let debug_dir = target_dir.join("debug");
    let bin_path = debug_dir.join("hello-rust");
    assert!(bin_path.exists(), "Executable must exist: {:?}", bin_path);

    // Verify observable behavior against M0 oracle
    let run_output = Command::new(&bin_path)
        .output()
        .expect("Failed to execute built binary");
    assert_eq!(run_output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&run_output.stdout);
    assert!(
        stdout.contains("Hello from Mitts fixture: 42"),
        "Unexpected output: {}",
        stdout
    );
}

#[test]
fn test_unsupported_case_explicitly_refused() {
    let manifest = fixture_manifest();
    let opt = RustBuildOptions {
        manifest_path: Some(manifest),
        extra_args: vec!["--unsupported-flag-xyz".to_string()],
        ..Default::default()
    };

    let err = RustAdapter::build(&opt).expect_err("Unsupported option must be refused");
    let err_msg = err.to_string();
    assert!(
        err_msg.contains("Unsupported build option"),
        "Expected unsupported option error, got: {}",
        err_msg
    );
}

#[test]
fn test_compile_error_preserves_failure_and_exit_code() {
    let temp_dir = tempfile::tempdir().expect("Failed to create tempdir");
    let temp_path = temp_dir.path();

    // Create broken Cargo crate
    let cargo_toml = r#"
[package]
name = "broken-crate"
version = "0.1.0"
edition = "2021"

[workspace]
"#;
    fs::write(temp_path.join("Cargo.toml"), cargo_toml).expect("write Cargo.toml");

    let src_dir = temp_path.join("src");
    fs::create_dir_all(&src_dir).expect("create src dir");
    let broken_src = "fn main() { let x: u32 = \"type mismatch\"; }";
    fs::write(src_dir.join("main.rs"), broken_src).expect("write main.rs");

    let opt = RustBuildOptions {
        manifest_path: Some(temp_path.join("Cargo.toml")),
        ..Default::default()
    };

    let (_, result) = RustAdapter::build(&opt).expect("Execution should return failure result");
    assert!(!result.success, "Compilation must report failure");
    assert_ne!(
        result.exit_code, 0,
        "Exit code must be non-zero on compiler error"
    );
    assert!(
        result.stderr.contains("mismatched types") || result.stdout.contains("mismatched types"),
        "Diagnostics must preserve compiler error"
    );
}

#[test]
fn test_coexistence_with_ordinary_cargo() {
    let tmp = tempfile::TempDir::new().unwrap();
    let orig_fixture = fixture_manifest();
    let fixture_dir = tmp.path().join("hello-rust");
    mitts::core::source_store::SourceStore::copy_dir_recursive(
        orig_fixture.parent().unwrap(),
        &fixture_dir,
    )
    .unwrap();
    let manifest = fixture_dir.join("Cargo.toml");

    // 1. Run ordinary Cargo build
    let cargo_status = Command::new("cargo")
        .arg("build")
        .arg("--manifest-path")
        .arg(&manifest)
        .status()
        .expect("Failed to run cargo build");
    assert!(cargo_status.success(), "Ordinary cargo build must succeed");

    let ordinary_target = fixture_dir.join("target");
    let ordinary_bin = ordinary_target.join("debug").join("hello-rust");
    assert!(ordinary_bin.exists(), "Ordinary cargo binary must exist");

    // 2. Run Mitts build with isolated .mitts/target
    let opt = RustBuildOptions {
        manifest_path: Some(manifest.clone()),
        ..Default::default()
    };
    let (plan, result) = RustAdapter::build(&opt).expect("Mitts build must succeed");
    assert!(result.success, "Mitts build must succeed");

    let mitts_target = fixture_dir.join(".mitts").join("target");
    assert_eq!(plan.target_directory, mitts_target);
    let mitts_bin = mitts_target.join("debug").join("hello-rust");
    assert!(mitts_bin.exists(), "Mitts binary must exist");

    // 3. Verify ordinary target was not deleted, overwritten, or corrupted
    assert!(
        ordinary_bin.exists(),
        "Ordinary target binary must remain intact"
    );

    // Both binaries must be independently executable with identical observable output
    let out_ordinary = Command::new(&ordinary_bin).output().expect("exec ordinary");
    let out_mitts = Command::new(&mitts_bin).output().expect("exec mitts");

    assert_eq!(out_ordinary.status.code(), Some(0));
    assert_eq!(out_mitts.status.code(), Some(0));
    assert_eq!(out_ordinary.stdout, out_mitts.stdout);
}

#[test]
fn test_explain_effective_plan() {
    let manifest = fixture_manifest();
    let opt = RustBuildOptions {
        manifest_path: Some(manifest.clone()),
        release: false,
        ..Default::default()
    };

    let explanation = RustAdapter::explain_plan(&opt).expect("Explain plan must succeed");
    assert_eq!(explanation.manifest_path, manifest);
    assert_eq!(explanation.profile, "debug");
    assert_eq!(
        explanation.target_directory,
        manifest.parent().unwrap().join(".mitts").join("target")
    );
    assert!(
        !explanation.units.is_empty(),
        "Plan must have compilation units"
    );

    let bin_unit = explanation
        .units
        .iter()
        .find(|u| u.package_name == "hello-rust" && u.kind == "bin");
    assert!(bin_unit.is_some(), "Must contain hello-rust binary unit");
    let bin_unit = bin_unit.unwrap();
    assert_eq!(bin_unit.target_name, "hello-rust");
    assert!(
        bin_unit.cache_eligibility.contains("Uncached in M1"),
        "Eligibility must state uncached in M1: {}",
        bin_unit.cache_eligibility
    );

    let dep_unit = explanation
        .units
        .iter()
        .find(|u| u.package_name == "itoa" && u.kind == "lib");
    assert!(dep_unit.is_some(), "Must contain itoa dependency unit");
    let dep_unit = dep_unit.unwrap();
    assert_eq!(dep_unit.target_name, "itoa");
    assert!(
        dep_unit.cache_eligibility.contains("eligible for M2"),
        "Eligibility must indicate M2 cache eligibility: {}",
        dep_unit.cache_eligibility
    );

    // Test with release mode
    let opt_release = RustBuildOptions {
        manifest_path: Some(manifest),
        release: true,
        ..Default::default()
    };
    let exp_release = RustAdapter::explain_plan(&opt_release).expect("Explain release plan");
    assert_eq!(exp_release.profile, "release");
}

#[test]
fn test_explain_unselected_workspace_member_excluded() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let ws_root = temp_dir.path();

    // Create root workspace Cargo.toml
    let ws_cargo = r#"
[workspace]
members = ["member_a", "member_b"]
"#;
    fs::write(ws_root.join("Cargo.toml"), ws_cargo).expect("write ws Cargo.toml");

    // Member A
    let dir_a = ws_root.join("member_a");
    fs::create_dir_all(dir_a.join("src")).expect("dir_a");
    fs::write(
        dir_a.join("Cargo.toml"),
        r#"
[package]
name = "member_a"
version = "0.1.0"
edition = "2021"
"#,
    )
    .expect("write a Cargo.toml");
    fs::write(dir_a.join("src").join("main.rs"), "fn main() {}").expect("write a main.rs");

    // Member B (independent)
    let dir_b = ws_root.join("member_b");
    fs::create_dir_all(dir_b.join("src")).expect("dir_b");
    fs::write(
        dir_b.join("Cargo.toml"),
        r#"
[package]
name = "member_b"
version = "0.1.0"
edition = "2021"
"#,
    )
    .expect("write b Cargo.toml");
    fs::write(dir_b.join("src").join("main.rs"), "fn main() {}").expect("write b main.rs");

    // Explain member_a specifically
    let opt = RustBuildOptions {
        manifest_path: Some(dir_a.join("Cargo.toml")),
        ..Default::default()
    };
    let exp = RustAdapter::explain_plan(&opt).expect("explain member_a");
    assert!(
        exp.units.iter().any(|u| u.package_name == "member_a"),
        "Must contain member_a"
    );
    assert!(
        !exp.units.iter().any(|u| u.package_name == "member_b"),
        "Must NOT contain unselected sibling member_b"
    );
}

#[test]
fn test_explain_mixed_crate_type_marked_uncached() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let ws_root = temp_dir.path();

    let cargo_toml = r#"
[package]
name = "mixed-lib"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["rlib", "cdylib"]

[workspace]
"#;
    fs::write(ws_root.join("Cargo.toml"), cargo_toml).expect("write Cargo.toml");
    fs::create_dir_all(ws_root.join("src")).expect("src");
    fs::write(ws_root.join("src").join("lib.rs"), "pub fn foo() {}").expect("lib.rs");

    let opt = RustBuildOptions {
        manifest_path: Some(ws_root.join("Cargo.toml")),
        ..Default::default()
    };
    let exp = RustAdapter::explain_plan(&opt).expect("explain mixed");
    let unit = exp
        .units
        .iter()
        .find(|u| u.package_name == "mixed-lib")
        .expect("find unit");
    assert!(
        unit.cache_eligibility
            .contains("outside initial M2 pure rlib subset"),
        "Mixed crate type must be marked uncached for initial M2: {}",
        unit.cache_eligibility
    );
}
