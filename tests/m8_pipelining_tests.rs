/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::rust::pipelining::{
    classify_dep_requirement, parse_rustc_artifact_line, parse_rustc_rendered_diagnostic,
    DepRequirement,
};
use mitts::adapter::{PlannedCompilationUnit, RustAdapter, RustBuildOptions};
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn make_dummy_unit(name: &str, kind: &str) -> PlannedCompilationUnit {
    PlannedCompilationUnit {
        package_id: name.to_string(),
        package_name: name.to_string(),
        version: "0.1.0".to_string(),
        target_name: name.to_string(),
        kind: kind.to_string(),
        src_path: PathBuf::from("src/lib.rs"),
        manifest_path: PathBuf::from("Cargo.toml"),
        cache_eligibility: "eligible".to_string(),
        is_workspace: true,
        is_primary: false,
        env_vars: Vec::new(),
        dependencies: Vec::new(),
        edition: Some("2021".to_string()),
        features: Vec::new(),
    }
}

#[test]
fn test_pipelining_dependency_classification() {
    let lib_a = make_dummy_unit("crate_a", "lib");
    let lib_b = make_dummy_unit("crate_b", "lib");
    let bin_c = make_dummy_unit("app", "bin");
    let test_d = make_dummy_unit("test_app", "test");
    let proc_macro_e = make_dummy_unit("macro_derive", "proc-macro");

    // Lib -> Lib is MetadataOnly
    assert_eq!(
        classify_dep_requirement(&lib_b, &lib_a),
        DepRequirement::MetadataOnly
    );

    // Bin -> Lib requires FullArtifact (machine code linking)
    assert_eq!(
        classify_dep_requirement(&bin_c, &lib_a),
        DepRequirement::FullArtifact
    );

    // Test -> Lib requires FullArtifact
    assert_eq!(
        classify_dep_requirement(&test_d, &lib_a),
        DepRequirement::FullArtifact
    );

    // Any unit depending on proc-macro requires FullArtifact (compiler host execution)
    assert_eq!(
        classify_dep_requirement(&lib_b, &proc_macro_e),
        DepRequirement::FullArtifact
    );
    assert_eq!(
        classify_dep_requirement(&bin_c, &proc_macro_e),
        DepRequirement::FullArtifact
    );
}

#[test]
fn test_parse_rustc_artifact_line() {
    let line_meta =
        r#"{"$message_type":"artifact","artifact":"/tmp/stage/libfoo.rmeta","emit":"metadata"}"#;
    let (path, emit) = parse_rustc_artifact_line(line_meta).expect("must parse metadata artifact");
    assert_eq!(path, PathBuf::from("/tmp/stage/libfoo.rmeta"));
    assert_eq!(emit, "metadata");

    let line_link =
        r#"{"$message_type":"artifact","artifact":"/tmp/stage/libfoo.rlib","emit":"link"}"#;
    let (path2, emit2) = parse_rustc_artifact_line(line_link).expect("must parse link artifact");
    assert_eq!(path2, PathBuf::from("/tmp/stage/libfoo.rlib"));
    assert_eq!(emit2, "link");

    let non_json = "warning: unused variable";
    assert!(parse_rustc_artifact_line(non_json).is_none());

    let other_json = r#"{"$message_type":"diagnostic","message":"unused variable"}"#;
    assert!(parse_rustc_artifact_line(other_json).is_none());
}

#[test]
fn test_parse_rustc_rendered_diagnostic() {
    let line_diag = r#"{"$message_type":"diagnostic","message":"unused variable","rendered":"warning: unused variable `x`\n"}"#;
    let rendered = parse_rustc_rendered_diagnostic(line_diag).expect("must parse diagnostic");
    assert!(rendered.contains("warning: unused variable `x`"));

    let non_diag = r#"{"$message_type":"artifact","artifact":"/path","emit":"metadata"}"#;
    assert!(parse_rustc_rendered_diagnostic(non_diag).is_none());
}

#[test]
fn test_pipelined_dag_build_e2e() {
    let temp_root = tempfile::tempdir().expect("temp root");
    let base = temp_root.path();

    // Create a 2-crate workspace: lib_core and app (binary depending on lib_core)
    let ws_dir = base.join("pipe_ws");
    fs::create_dir_all(ws_dir.join("src")).unwrap();

    let cargo_toml = r#"
[package]
name = "pipe-app"
version = "0.1.0"
edition = "2021"

[dependencies]
itoa = "1.0.14"
"#;
    fs::write(ws_dir.join("Cargo.toml"), cargo_toml).unwrap();
    fs::write(
        ws_dir.join("src").join("main.rs"),
        "fn main() { println!(\"pipe: {}\", itoa::Buffer::new().format(42)); }",
    )
    .unwrap();

    let cas_dir = base.join("cas");
    let opt = RustBuildOptions {
        manifest_path: Some(ws_dir.join("Cargo.toml")),
        cache_dir: Some(cas_dir),
        target_dir: Some(ws_dir.join("target")),
        native: true,
        ..Default::default()
    };

    let (_, res) = RustAdapter::build_native(&opt).expect("native dag build with pipelining");
    assert!(res.success, "Build must succeed: {}", res.stderr);

    let bin_path = ws_dir.join("target").join("debug").join("pipe-app");
    assert!(bin_path.is_file(), "Binary must be produced");

    let out = Command::new(&bin_path).output().expect("execute binary");
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("pipe: 42"));
}
