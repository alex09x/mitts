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
use mitts::adapter::rust::types::PlannedCompilationUnit;
use mitts::adapter::BuildMode;
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

#[test]
fn test_m8_compiler_clippy_driver_and_modes() {
    // 1. resolve_clippy_driver with env var override
    let old = std::env::var("CLIPPY_DRIVER").ok();
    std::env::set_var("CLIPPY_DRIVER", "/custom/bin/clippy-driver");
    let resolved = resolve_clippy_driver();
    assert_eq!(resolved, PathBuf::from("/custom/bin/clippy-driver"));
    if let Some(o) = old {
        std::env::set_var("CLIPPY_DRIVER", o);
    } else {
        std::env::remove_var("CLIPPY_DRIVER");
    }

    let temp_dir = TempDir::new().expect("tempdir");
    let staging_dir = temp_dir.path().join("staging");
    fs::create_dir_all(&staging_dir).unwrap();

    let manifest = temp_dir.path().join("Cargo.toml");
    fs::write(
        &manifest,
        "[package]\nname = \"comp-pkg\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();

    let src = temp_dir.path().join("lib.rs");
    fs::write(&src, "pub fn add(a: i32, b: i32) -> i32 { a + b }").unwrap();

    // 2. Proc-macro compilation
    let proc_unit = PlannedCompilationUnit {
        package_id: "my-macro@0.1.0".to_string(),
        package_name: "my-macro".to_string(),
        version: "0.1.0".to_string(),
        target_name: "my_macro".to_string(),
        kind: "proc-macro".to_string(),
        src_path: src.clone(),
        manifest_path: manifest.clone(),
        dependencies: Vec::new(),
        edition: Some("2021".to_string()),
        is_workspace: true,
        cache_eligibility: "cacheable".to_string(),
        env_vars: Vec::new(),
        features: Vec::new(),
    };

    let proc_inv = RustcInvocation {
        unit: &proc_unit,
        action_key: "0123456789abcdef0123456789abcdef",
        out_dir: &staging_dir,
        dest_binary: None,
        extern_crates: &[],
        release: false,
        features: &[],
        directives: None,
        mode: BuildMode::Build,
        extra_compiler_args: &[],
    };
    let _ = proc_inv.execute();

    // 3. Bin unit with dest_binary = None (covers out_dir branch)
    let bin_src = temp_dir.path().join("main.rs");
    fs::write(&bin_src, "fn main() {}").unwrap();
    let bin_unit = PlannedCompilationUnit {
        package_id: "my-bin@0.1.0".to_string(),
        package_name: "my-bin".to_string(),
        version: "0.1.0".to_string(),
        target_name: "my_bin".to_string(),
        kind: "bin".to_string(),
        src_path: bin_src,
        manifest_path: manifest.clone(),
        dependencies: Vec::new(),
        edition: Some("2021".to_string()),
        is_workspace: true,
        cache_eligibility: "cacheable".to_string(),
        env_vars: Vec::new(),
        features: Vec::new(),
    };
    let bin_inv = RustcInvocation {
        unit: &bin_unit,
        action_key: "fedcba9876543210fedcba9876543210",
        out_dir: &staging_dir,
        dest_binary: None,
        extern_crates: &[],
        release: false,
        features: &[],
        directives: None,
        mode: BuildMode::Build,
        extra_compiler_args: &[],
    };
    let _ = bin_inv.execute();

    // 4. Bin unit in Check mode (covers emit=dep-info,metadata branch)
    let check_inv = RustcInvocation {
        unit: &bin_unit,
        action_key: "11223344556677881122334455667788",
        out_dir: &staging_dir,
        dest_binary: None,
        extern_crates: &[],
        release: false,
        features: &[],
        directives: None,
        mode: BuildMode::Check,
        extra_compiler_args: &[],
    };
    let _ = check_inv.execute();
}

#[test]
fn test_m8_build_script_directives_and_parse_stdout() {
    let mut empty = BuildScriptDirectives::default();
    assert!(empty.is_empty());

    empty.cfgs.push("has_feature".to_string());
    assert!(!empty.is_empty());

    // dep_env_vars with links
    let mut with_links = BuildScriptDirectives::default();
    with_links.links = Some("zstd-sys".to_string());
    with_links
        .metadata
        .insert("include".to_string(), "/usr/include".to_string());
    with_links
        .metadata
        .insert("lib-version".to_string(), "1.5.0".to_string());

    let dep_vars = with_links.dep_env_vars();
    assert_eq!(
        dep_vars.get("DEP_ZSTD_SYS_INCLUDE"),
        Some(&"/usr/include".to_string())
    );
    assert_eq!(
        dep_vars.get("DEP_ZSTD_SYS_LIB_VERSION"),
        Some(&"1.5.0".to_string())
    );

    // parse_stdout comprehensive test
    let stdout = r#"
cargo:rustc-cfg=custom_feature
cargo:rustc-link-lib=static=foo
cargo:rustc-link-search=native=/usr/local/lib
cargo:rustc-link-search=/opt/lib
cargo:rustc-env=TARGET_PLATFORM=x86_64
cargo:custom_meta=val_meta
cargo::metadata=key_two=val_two
cargo:warning=this is a build warning
cargo:rustc-check-cfg=cfg(has_foo)
cargo:rustc-flags=-L /extra
cargo:rerun-if-changed=build.rs
"#;

    let parsed = BuildScriptDirectives::parse_stdout(stdout);
    assert!(parsed.cfgs.contains(&"custom_feature".to_string()));
    assert!(parsed.link_libs.contains(&"static=foo".to_string()));
    assert!(parsed.link_searches.contains(&"/usr/local/lib".to_string()));
    assert!(parsed.link_searches.contains(&"/opt/lib".to_string()));
    assert_eq!(
        parsed.envs.get("TARGET_PLATFORM"),
        Some(&"x86_64".to_string())
    );
    assert_eq!(parsed.metadata.get("custom_meta"), Some(&"val_meta".to_string()));
    assert_eq!(parsed.metadata.get("key_two"), Some(&"val_two".to_string()));
    assert!(!parsed.metadata.contains_key("warning"));
    assert!(!parsed.metadata.contains_key("rerun-if-changed"));
}

#[test]
fn test_m8_build_script_execution_disabled_nested_and_nonsemver() {
    let temp_dir = TempDir::new().expect("tempdir");
    let pkg_dir = temp_dir.path().join("my_pkg");
    let staging_dir = temp_dir.path().join("staging");
    fs::create_dir_all(&pkg_dir).unwrap();
    fs::create_dir_all(&staging_dir).unwrap();

    let manifest = pkg_dir.join("Cargo.toml");

    // 1. Build script explicitly disabled: build = false
    fs::write(
        &manifest,
        "[package]\nname = \"pkg-dis\"\nversion = \"0.1.0\"\nbuild = false\n",
    )
    .unwrap();

    let unit = PlannedCompilationUnit {
        package_id: "pkg-dis@0.1.0".to_string(),
        package_name: "pkg-dis".to_string(),
        version: "0.1.0".to_string(),
        target_name: "pkg_dis".to_string(),
        kind: "lib".to_string(),
        src_path: pkg_dir.join("src").join("lib.rs"),
        manifest_path: manifest.clone(),
        dependencies: Vec::new(),
        edition: Some("2021".to_string()),
        is_workspace: true,
        cache_eligibility: "cacheable".to_string(),
        env_vars: Vec::new(),
        features: Vec::new(),
    };

    let dir = execute_build_script(&unit, &staging_dir, false, &[], None, &[], None).unwrap();
    assert!(dir.is_empty());

    // 2. Nested build script: build/build.rs
    let nested_dir = pkg_dir.join("build");
    fs::create_dir_all(&nested_dir).unwrap();
    fs::write(
        nested_dir.join("build.rs"),
        "fn main() { println!(\"cargo:rustc-cfg=nested_ok\"); }\n",
    )
    .unwrap();
    fs::write(
        &manifest,
        "[package]\nname = \"pkg-nested\"\nversion = \"1.2-beta\"\n",
    )
    .unwrap();

    let mut nested_unit = unit;
    nested_unit.package_name = "pkg-nested".to_string();
    nested_unit.version = "1.2-beta".to_string();

    let nested_dir_res =
        execute_build_script(&nested_unit, &staging_dir, false, &[], None, &[], None).unwrap();
    assert!(nested_dir_res.cfgs.contains(&"nested_ok".to_string()));
}
