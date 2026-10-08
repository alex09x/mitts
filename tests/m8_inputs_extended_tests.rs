/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::rust::inputs::{append_unit_dep_info_inputs, hash_package_base_inputs};
use std::fs;
use tempfile::TempDir;

#[test]
fn test_m8_inputs_ancestors_and_nested_configs() {
    let tmp = TempDir::new().unwrap();
    let ws_root = tmp.path().join("workspace");
    let intermediate = ws_root.join("crates");
    let pkg_dir = intermediate.join("my-pkg");
    fs::create_dir_all(&pkg_dir).unwrap();

    let root_toml = ws_root.join("Cargo.toml");
    fs::write(&root_toml, "[workspace]\nmembers = [\"crates/my-pkg\"]\n").unwrap();

    let intermediate_toml = intermediate.join("Cargo.toml");
    fs::write(&intermediate_toml, "[workspace]\n").unwrap();

    let pkg_toml = pkg_dir.join("Cargo.toml");
    fs::write(
        &pkg_toml,
        "[package]\nname = \"my-pkg\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();

    let ws_lock = ws_root.join("Cargo.lock");
    fs::write(&ws_lock, "version = 3\n").unwrap();

    // Intermediate .cargo/config.toml
    let inter_cargo = intermediate.join(".cargo");
    fs::create_dir_all(&inter_cargo).unwrap();
    fs::write(inter_cargo.join("config.toml"), "[build]\n").unwrap();
    fs::write(inter_cargo.join("config"), "[target]\n").unwrap();

    // Package .cargo/config.toml
    let pkg_cargo = pkg_dir.join(".cargo");
    fs::create_dir_all(&pkg_cargo).unwrap();
    fs::write(pkg_cargo.join("config.toml"), "[env]\n").unwrap();
    fs::write(pkg_cargo.join("config"), "[term]\n").unwrap();

    let inputs = hash_package_base_inputs(&pkg_dir, Some(&root_toml), true).unwrap();
    assert!(inputs.iter().any(|(k, _)| k == "workspace:Cargo.toml"));
    assert!(inputs.iter().any(|(k, _)| k.starts_with("ancestor:")));
    assert!(inputs.iter().any(|(k, _)| k == "Cargo.lock"));
    assert!(inputs.iter().any(|(k, _)| k == ".cargo/config.toml"));
    assert!(inputs.iter().any(|(k, _)| k == ".cargo/config"));
    assert!(inputs
        .iter()
        .any(|(k, _)| k.contains("ancestor:") && k.contains("config")));
}

#[test]
fn test_m8_inputs_cargo_home_and_file_filters() {
    let tmp = TempDir::new().unwrap();
    let pkg_dir = tmp.path().join("pkg");
    fs::create_dir_all(pkg_dir.join("src")).unwrap();

    let pkg_toml = pkg_dir.join("Cargo.toml");
    fs::write(
        &pkg_toml,
        "[package]\nname = \"filter-test\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(pkg_dir.join("src").join("lib.rs"), "pub fn ok() {}\n").unwrap();

    // Files that MUST be ignored
    fs::write(pkg_dir.join("temp.tmp"), "tmp").unwrap();
    fs::write(pkg_dir.join("swap.swp"), "swp").unwrap();
    fs::write(pkg_dir.join("old.bak"), "bak").unwrap();
    fs::write(pkg_dir.join("local.lock"), "lock").unwrap();

    // Build and VCS dirs are ignored
    let target_dir = pkg_dir.join("target");
    fs::create_dir_all(&target_dir).unwrap();
    fs::write(target_dir.join("artifact"), "bin").unwrap();

    let git_dir = pkg_dir.join(".git");
    fs::create_dir_all(&git_dir).unwrap();
    fs::write(git_dir.join("HEAD"), "ref").unwrap();

    // Package asset directories (traces, cts, terraform) are included
    let traces_dir = pkg_dir.join("traces");
    fs::create_dir_all(&traces_dir).unwrap();
    fs::write(traces_dir.join("trace.json"), "{}").unwrap();

    let cts_dir = pkg_dir.join("cts");
    fs::create_dir_all(&cts_dir).unwrap();
    fs::write(cts_dir.join("data.bin"), "bin").unwrap();

    let tf_dir = pkg_dir.join("terraform");
    fs::create_dir_all(&tf_dir).unwrap();
    fs::write(tf_dir.join("main.tf"), "tf").unwrap();

    // CARGO_HOME configs
    let fake_cargo_home = tmp.path().join("cargo_home");
    fs::create_dir_all(&fake_cargo_home).unwrap();
    fs::write(fake_cargo_home.join("config.toml"), "[net]\n").unwrap();
    fs::write(fake_cargo_home.join("config"), "[alias]\n").unwrap();

    let old_cargo_home = std::env::var("CARGO_HOME").ok();
    std::env::set_var("CARGO_HOME", &fake_cargo_home);

    let inputs = hash_package_base_inputs(&pkg_dir, None, false).unwrap();

    if let Some(h) = old_cargo_home {
        std::env::set_var("CARGO_HOME", h);
    } else {
        std::env::remove_var("CARGO_HOME");
    }

    assert!(inputs.iter().any(|(k, _)| k == "CARGO_HOME/config.toml"));
    assert!(inputs.iter().any(|(k, _)| k == "CARGO_HOME/config"));
    assert!(inputs.iter().any(|(k, _)| k == "src/lib.rs"));

    assert!(!inputs.iter().any(|(k, _)| k.ends_with(".tmp")));
    assert!(!inputs.iter().any(|(k, _)| k.ends_with(".swp")));
    assert!(!inputs.iter().any(|(k, _)| k.starts_with("target/")));
    assert!(!inputs.iter().any(|(k, _)| k.starts_with(".git/")));
    assert!(inputs.iter().any(|(k, _)| k.starts_with("traces/")));
    assert!(inputs.iter().any(|(k, _)| k.starts_with("cts/")));
    assert!(inputs.iter().any(|(k, _)| k.starts_with("terraform/")));
}

#[test]
fn test_m8_inputs_dep_info_out_of_package() {
    let tmp = TempDir::new().unwrap();
    let pkg_dir = tmp.path().join("pkg");
    fs::create_dir_all(&pkg_dir).unwrap();

    let outside_dir = tmp.path().join("shared");
    fs::create_dir_all(&outside_dir).unwrap();
    let outside_rs = outside_dir.join("shared_module.rs");
    fs::write(&outside_rs, "pub fn shared() {}\n").unwrap();

    let target_dir = tmp.path().join("target");
    let debug_deps = target_dir.join("debug").join("deps");
    fs::create_dir_all(&debug_deps).unwrap();

    let d_file = debug_deps.join("my_lib.d");
    fs::write(
        &d_file,
        format!("/target/libmy_lib.rlib: {}\n", outside_rs.display()),
    )
    .unwrap();

    let base_inputs = vec![("Cargo.toml".to_string(), "hash_toml".to_string())];
    let appended =
        append_unit_dep_info_inputs(base_inputs, &pkg_dir, &target_dir, "my_lib").unwrap();

    assert!(appended.iter().any(|(k, _)| k.contains("shared_module.rs")));
}
