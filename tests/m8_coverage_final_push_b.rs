/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::rust::build_script::{execute_build_script, BuildScriptDirectives};
use mitts::adapter::rust::compiler::{resolve_clippy_driver, RustcInvocation};
use mitts::adapter::rust::inputs::hash_package_base_inputs;
use mitts::adapter::rust::resolver::NativeResolver;
use mitts::adapter::rust::sparse_index::SparseIndexClient;
use mitts::adapter::rust::types::{BuildMode, PlannedCompilationUnit};
use std::fs;

#[test]
fn test_m8_build_script_directives_and_release_execution() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    // 1. BuildScriptDirectives helpers
    let mut dir = BuildScriptDirectives::default();
    assert!(dir.is_empty());
    dir.links = Some("sys_zlib".into());
    dir.metadata.insert("include".into(), "/usr/include".into());
    assert!(!dir.is_empty());
    let env_vars = dir.dep_env_vars();
    assert_eq!(
        env_vars.get("DEP_SYS_ZLIB_INCLUDE"),
        Some(&"/usr/include".to_string())
    );

    // 2. execute_build_script with release: true
    let manifest = root.join("Cargo.toml");
    fs::write(
        &manifest,
        r#"
[package]
name = "release-build-script"
version = "1.2.3"
edition = "2021"
links = "custom_lib"
"#,
    )
    .unwrap();
    fs::write(
        root.join("build.rs"),
        r#"
fn main() {
    println!("cargo:rustc-cfg=release_mode");
    println!("cargo:metadata=lib_dir=/opt/lib");
}
"#,
    )
    .unwrap();

    let unit = PlannedCompilationUnit {
        package_id: "release-build-script@1.2.3".into(),
        package_name: "release-build-script".into(),
        version: "1.2.3".into(),
        target_name: "release_build_script".into(),
        kind: "lib".into(),
        src_path: root.join("src").join("lib.rs"),
        manifest_path: manifest,
        cache_eligibility: "Workspace".into(),
        is_workspace: true,
        is_primary: false,
        env_vars: Vec::new(),
        dependencies: Vec::new(),
        edition: Some("2021".into()),
        features: Vec::new(),
        for_host: false,
        workspace_root: None,
    };

    let staging = root.join("staging");
    fs::create_dir_all(&staging).unwrap();
    let res = execute_build_script(
        &unit,
        &staging,
        true, // release = true (covers OPT_LEVEL=3, DEBUG=false)
        &["custom_feat".into()],
        None,
        &[],
        None,
    );
    assert!(res.is_ok(), "build script should succeed: {:?}", res.err());
    let directives = res.unwrap();
    assert!(directives.cfgs.contains(&"release_mode".to_string()));
    assert_eq!(directives.links, Some("custom_lib".to_string()));
}

#[test]
fn test_m8_resolver_client_and_detailed_deps() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    // 1. NativeResolver with custom client
    let index = SparseIndexClient::with_cache_dir(root.join("index_cache")).unwrap();
    let resolver = NativeResolver::with_client(index);

    // 2. Resolve manifest with local path dep
    let manifest = root.join("Cargo.toml");
    fs::write(
        &manifest,
        r#"
[package]
name = "detailed-remote-dep"
version = "0.5.0"
edition = "2021"

[dependencies]
local-pkg = { path = "./local" }
"#,
    )
    .unwrap();
    let local_dir = root.join("local");
    fs::create_dir_all(&local_dir).unwrap();
    fs::write(
        local_dir.join("Cargo.toml"),
        r#"
[package]
name = "local-pkg"
version = "0.2.0"
edition = "2021"
"#,
    )
    .unwrap();

    let lock = resolver.resolve_manifest(&manifest, true).unwrap();
    assert!(lock.packages.iter().any(|p| p.name == "local-pkg"));
    assert!(lock
        .packages
        .iter()
        .any(|p| p.name == "detailed-remote-dep"));
}

#[test]
fn test_m8_compiler_dest_binary_none_and_clippy_driver() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    // 1. resolve_clippy_driver fallback
    let driver = resolve_clippy_driver();
    assert!(!driver.as_os_str().is_empty());

    // 2. RustcInvocation with is_bin = true and dest_binary = None
    let manifest = root.join("Cargo.toml");
    fs::write(
        &manifest,
        r#"
[package]
name = "bin-none-dest"
version = "0.1.0"
edition = "2021"
"#,
    )
    .unwrap();
    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir).unwrap();
    let main_rs = src_dir.join("main.rs");
    fs::write(&main_rs, "fn main() {}\n").unwrap();

    let out_dir = root.join("out");
    fs::create_dir_all(&out_dir).unwrap();

    let unit = PlannedCompilationUnit {
        package_id: "bin-none-dest@0.1.0".into(),
        package_name: "bin-none-dest".into(),
        version: "0.1.0".into(),
        target_name: "bin_none_dest".into(),
        kind: "bin".into(),
        src_path: main_rs,
        manifest_path: manifest,
        cache_eligibility: "Workspace".into(),
        is_workspace: true,
        is_primary: false,
        env_vars: Vec::new(),
        dependencies: Vec::new(),
        edition: Some("2021".into()),
        features: Vec::new(),
        for_host: false,
        workspace_root: None,
    };

    let invocation = RustcInvocation {
        unit: &unit,
        action_key: "shortkey123",
        out_dir: &out_dir,
        dest_binary: None, // dest_binary is None (covers out_dir destination)
        extern_crates: &[],
        release: false,
        features: &[],
        directives: None,
        mode: BuildMode::Build,
        extra_compiler_args: &[],
        jobserver_auth: None,
        deps_dir: None,
        target_dir: None,
        cache_root: None,
    };

    let res = invocation.execute();
    assert!(res.is_ok(), "invocation should succeed: {:?}", res.err());
    let out = res.unwrap();
    assert!(out.success);
    assert!(out.primary_output.is_some());
}

#[test]
fn test_m8_inputs_immutable_registry_without_cargo_toml() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    // Directory marked with .cargo-ok but WITHOUT Cargo.toml
    let pkg_dir = root.join("immutable_no_toml");
    fs::create_dir_all(&pkg_dir).unwrap();
    fs::write(pkg_dir.join(".cargo-ok"), "").unwrap();

    // Outside $CARGO_HOME/registry/src the marker is ignored, so the missing
    // Cargo.toml is an error rather than an immutable fast path.
    let res = hash_package_base_inputs(&pkg_dir, None, false);
    assert!(res.is_err());
}
