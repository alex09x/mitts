/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::{CompilationDag, RustAdapter, RustBuildOptions};
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
fn test_m4_compilation_dag_structure() {
    let manifest = fixture_manifest();
    let opt = RustBuildOptions {
        manifest_path: Some(manifest.clone()),
        ..Default::default()
    };

    let explanation = RustAdapter::explain_plan(&opt).expect("explain plan");
    let dag = CompilationDag::from_explanation(&explanation).expect("build DAG");

    assert!(!dag.is_empty(), "DAG must contain nodes");
    assert!(dag.len() >= 2, "DAG must contain at least itoa and hello-rust");

    // itoa must be a leaf unit (no dependencies)
    let leaf_nodes: Vec<_> = dag
        .leaf_ids()
        .iter()
        .filter_map(|id| dag.get_node(id))
        .collect();
    assert!(
        leaf_nodes.iter().any(|n| n.unit.package_name == "itoa"),
        "itoa must be a leaf unit ready to compile"
    );

    // hello-rust must be a root unit (no dependents)
    let root_nodes: Vec<_> = dag
        .root_ids()
        .iter()
        .filter_map(|id| dag.get_node(id))
        .collect();
    assert!(
        root_nodes.iter().any(|n| n.unit.package_name == "hello-rust"),
        "hello-rust must be a root unit"
    );

    // Topological waves: Wave 0 must contain itoa, later wave must contain hello-rust
    let waves = dag.topological_waves().expect("topological waves");
    assert!(waves.len() >= 2, "Must have at least 2 topological waves");

    let wave0_packages: Vec<_> = waves[0]
        .iter()
        .filter_map(|id| dag.get_node(id))
        .map(|n| n.unit.package_name.as_str())
        .collect();
    assert!(
        wave0_packages.contains(&"itoa"),
        "Wave 0 must contain leaf dependency itoa"
    );

    let last_wave_packages: Vec<_> = waves.last().unwrap()
        .iter()
        .filter_map(|id| dag.get_node(id))
        .map(|n| n.unit.package_name.as_str())
        .collect();
    assert!(
        last_wave_packages.contains(&"hello-rust"),
        "Final wave must contain root binary hello-rust"
    );
}

#[test]
fn test_m4_native_dag_build_end_to_end_and_zero_target_pollution() {
    let temp_cache = tempfile::tempdir().expect("cache tempdir");
    let temp_target = tempfile::tempdir().expect("target tempdir");
    let target_dir = temp_target.path().to_path_buf();

    let manifest = fixture_manifest();

    let opt = RustBuildOptions {
        manifest_path: Some(manifest.clone()),
        cache_dir: Some(temp_cache.path().to_path_buf()),
        target_dir: Some(target_dir.clone()),
        native: true,
        ..Default::default()
    };

    // 1. Cold native DAG build directly invoking rustc
    let (plan, result) = RustAdapter::build_native(&opt).expect("Native DAG build must succeed");
    assert!(result.success, "Native build must succeed: {}", result.stderr);
    assert_eq!(result.exit_code, 0);
    assert!(!result.cached, "Cold build must not be marked cached");
    assert!(!plan.actions.is_empty(), "Plan must contain recorded actions");

    // 2. Output binary verification
    let bin_path = target_dir.join("debug").join("hello-rust");
    assert!(bin_path.is_file(), "Executable must exist: {:?}", bin_path);

    let run_output = Command::new(&bin_path)
        .output()
        .expect("Failed to execute native built binary");
    assert_eq!(run_output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&run_output.stdout);
    assert!(
        stdout.contains("Hello from Mitts fixture: 42"),
        "Unexpected binary output: {}",
        stdout
    );

    // 3. ZERO INTERMEDIATE TARGET POLLUTION verification:
    // Dependencies (.rlib/.rmeta) must live solely in Global CAS, not in local target_dir!
    let deps_dir = target_dir.join("debug").join("deps");
    if deps_dir.is_dir() {
        let intermediate_rlibs: Vec<_> = fs::read_dir(&deps_dir)
            .expect("read deps")
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.path()
                    .extension()
                    .map(|ext| ext == "rlib" || ext == "rmeta")
                    .unwrap_or(false)
            })
            .collect();
        assert!(
            intermediate_rlibs.is_empty(),
            "Workspace target directory must have ZERO intermediate .rlib/.rmeta pollution! Found: {:?}",
            intermediate_rlibs
        );
    }

    // Verify artifacts live in the Content Addressed Store
    let cas_artifacts = temp_cache.path().join("cas").join("artifacts");
    assert!(
        cas_artifacts.is_dir(),
        "CAS artifacts dir must exist in global store"
    );
    let artifact_entries: Vec<_> = fs::read_dir(&cas_artifacts)
        .expect("read cas artifacts")
        .filter_map(|e| e.ok())
        .collect();
    assert!(
        !artifact_entries.is_empty(),
        "CAS artifacts dir must contain compiled dependency rlibs"
    );
    assert!(
        artifact_entries.iter().any(|e| {
            e.file_name().to_string_lossy().starts_with("libitoa")
        }),
        "CAS must contain libitoa rlib"
    );

    // 4. Warm native build: 100% cache hit
    let (plan_warm, result_warm) =
        RustAdapter::build_native(&opt).expect("Warm native build must succeed");
    assert!(result_warm.success);
    assert_eq!(result_warm.exit_code, 0);
    assert!(result_warm.cached, "Warm build must be marked cached");
    assert!(!plan_warm.actions.is_empty());
}

#[test]
fn test_m4_native_dag_compiler_diagnostic_preservation() {
    let temp_cache = tempfile::tempdir().expect("cache tempdir");
    let temp_target = tempfile::tempdir().expect("target tempdir");
    let temp_proj = tempfile::tempdir().expect("project tempdir");

    let cargo_toml = temp_proj.path().join("Cargo.toml");
    let src_dir = temp_proj.path().join("src");
    fs::create_dir_all(&src_dir).expect("create src dir");
    let main_rs = src_dir.join("main.rs");

    fs::write(
        &cargo_toml,
        r#"
[package]
name = "broken-app"
version = "0.1.0"
edition = "2021"
"#,
    )
    .expect("write Cargo.toml");

    // Syntax error in main.rs
    fs::write(
        &main_rs,
        r#"
fn main() {
    let x: u32 = "not a number";
}
"#,
    )
    .expect("write main.rs");

    let opt = RustBuildOptions {
        manifest_path: Some(cargo_toml),
        cache_dir: Some(temp_cache.path().to_path_buf()),
        target_dir: Some(temp_target.path().to_path_buf()),
        native: true,
        ..Default::default()
    };

    let (_plan, result) = RustAdapter::build_native(&opt).expect("Runner returns ActionResult");
    assert!(!result.success, "Build must fail on type error");
    assert_ne!(result.exit_code, 0, "Exit code must be non-zero");
    assert!(
        result.stderr.contains("mismatched types")
            || result.stderr.contains("expected `u32`, found `&str`"),
        "Diagnostic stderr must preserve compiler error, got: {}",
        result.stderr
    );
}

#[test]
fn test_m4_native_dag_multi_crate_parallel_waves() {
    let temp_cache = tempfile::tempdir().expect("cache tempdir");
    let temp_target = tempfile::tempdir().expect("target tempdir");
    let temp_workspace = tempfile::tempdir().expect("workspace tempdir");

    let ws_root = temp_workspace.path();
    let ws_cargo_toml = ws_root.join("Cargo.toml");
    fs::write(
        &ws_cargo_toml,
        r#"
[workspace]
members = ["crate-a", "crate-b", "app"]
resolver = "2"
"#,
    )
    .expect("write workspace Cargo.toml");

    // crate-a (leaf 1)
    let a_dir = ws_root.join("crate-a");
    fs::create_dir_all(a_dir.join("src")).unwrap();
    fs::write(
        a_dir.join("Cargo.toml"),
        r#"
[package]
name = "crate-a"
version = "0.1.0"
edition = "2021"
"#,
    )
    .unwrap();
    fs::write(
        a_dir.join("src").join("lib.rs"),
        "pub fn val_a() -> i32 { 10 }",
    )
    .unwrap();

    // crate-b (leaf 2)
    let b_dir = ws_root.join("crate-b");
    fs::create_dir_all(b_dir.join("src")).unwrap();
    fs::write(
        b_dir.join("Cargo.toml"),
        r#"
[package]
name = "crate-b"
version = "0.1.0"
edition = "2021"
"#,
    )
    .unwrap();
    fs::write(
        b_dir.join("src").join("lib.rs"),
        "pub fn val_b() -> i32 { 20 }",
    )
    .unwrap();

    // app (depends on crate-a and crate-b)
    let app_dir = ws_root.join("app");
    fs::create_dir_all(app_dir.join("src")).unwrap();
    fs::write(
        app_dir.join("Cargo.toml"),
        r#"
[package]
name = "app"
version = "0.1.0"
edition = "2021"

[dependencies]
crate-a = { path = "../crate-a" }
crate-b = { path = "../crate-b" }
"#,
    )
    .unwrap();
    fs::write(
        app_dir.join("src").join("main.rs"),
        r#"
fn main() {
    let sum = crate_a::val_a() + crate_b::val_b();
    println!("Computed sum: {}", sum);
}
"#,
    )
    .unwrap();

    let opt = RustBuildOptions {
        manifest_path: Some(app_dir.join("Cargo.toml")),
        cache_dir: Some(temp_cache.path().to_path_buf()),
        target_dir: Some(temp_target.path().to_path_buf()),
        native: true,
        ..Default::default()
    };

    let (_plan, result) = RustAdapter::build_native(&opt).expect("Multi-crate build must succeed");
    assert!(result.success, "Multi-crate build must succeed: {}", result.stderr);
    assert_eq!(result.exit_code, 0);

    let bin_path = temp_target.path().join("debug").join("app");
    assert!(bin_path.is_file(), "Executable app must exist: {:?}", bin_path);

    let run_output = Command::new(&bin_path)
        .output()
        .expect("Failed to execute multi-crate built app");
    assert_eq!(run_output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&run_output.stdout);
    assert!(
        stdout.contains("Computed sum: 30"),
        "Unexpected output: {}",
        stdout
    );
}
