/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use flate2::write::GzEncoder;
use flate2::Compression;
use mitts::adapter::{BuildMode, RustAdapter, RustBuildOptions};
use mitts::core::source_store::SourceStore;
use std::fs;
use std::sync::Mutex;
use tar::Builder;
use tempfile::TempDir;

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn make_tar_gz(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut enc = GzEncoder::new(Vec::new(), Compression::default());
    {
        let mut tar = Builder::new(&mut enc);
        for (name, content) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(content.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append_data(&mut header, *name, *content).unwrap();
        }
        tar.finish().unwrap();
    }
    enc.finish().unwrap()
}

#[test]
fn test_m8_planner_sync_lockfile_and_bin_self_lib_dependency() {
    let isolated_cache = tempfile::tempdir().expect("isolated cache dir");
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = TempDir::new().unwrap();
    let pkg_dir = temp.path().join("dual_target_pkg");
    fs::create_dir_all(pkg_dir.join("src").join("bin")).unwrap();

    // Package with both library and binary, but NO Cargo.lock on disk
    fs::write(
        pkg_dir.join("Cargo.toml"),
        br#"[package]
name = "dual_target_pkg"
version = "0.3.0"
edition = "2021"

[dependencies]
"#,
    )
    .unwrap();

    fs::write(
        pkg_dir.join("src").join("lib.rs"),
        b"pub fn get_num() -> i32 { 100 }\n",
    )
    .unwrap();

    fs::write(
        pkg_dir.join("src").join("bin").join("main.rs"),
        b"fn main() { println!(\"{}\", dual_target_pkg::get_num()); }\n",
    )
    .unwrap();

    let manifest_path = pkg_dir.join("Cargo.toml");
    let opt = RustBuildOptions {
        manifest_path: Some(manifest_path.clone()),
        package: None,
        release: false,
        target_dir: None,
        features: Vec::new(),
        no_default_features: false,
        all_features: false,
        no_cache: false,
        cache_dir: Some(isolated_cache.path().to_path_buf()),
        mode: BuildMode::Build,
        native: true,
        cargo: false,
        extra_args: Vec::new(),
        verbose: 0,
        progress: None,
        ..Default::default()
    };

    // Plans without existing Cargo.lock -> resolves lockfile and plans units
    let plan = RustAdapter::explain_plan(&opt).unwrap();
    assert_eq!(plan.units.len(), 2);

    let bin_unit = plan.units.iter().find(|u| u.kind == "bin").unwrap();
    let lib_unit = plan.units.iter().find(|u| u.kind == "lib").unwrap();

    // Verify bin unit automatically depends on self lib unit
    assert!(bin_unit.dependencies.contains(&lib_unit.unit_id()));
}

#[test]
fn test_m8_planner_path_dep_injection_into_lockfile() {
    let isolated_cache = tempfile::tempdir().expect("isolated cache dir");
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = TempDir::new().unwrap();
    let root_dir = temp.path().join("root_pkg");
    let sub_dir = temp.path().join("sub_lib");
    fs::create_dir_all(root_dir.join("src")).unwrap();
    fs::create_dir_all(sub_dir.join("src")).unwrap();

    fs::write(
        sub_dir.join("Cargo.toml"),
        br#"[package]
name = "sub_lib"
version = "0.1.0"
edition = "2021"
"#,
    )
    .unwrap();
    fs::write(sub_dir.join("src").join("lib.rs"), b"pub fn helper() {}\n").unwrap();

    fs::write(
        root_dir.join("Cargo.toml"),
        br#"[package]
name = "root_pkg"
version = "0.1.0"
edition = "2021"

[dependencies]
sub_lib = { path = "../sub_lib" }
"#,
    )
    .unwrap();
    fs::write(root_dir.join("src").join("lib.rs"), b"pub fn run() {}\n").unwrap();

    // Minimal lockfile that omits sub_lib to test path dependency injection
    fs::write(
        root_dir.join("Cargo.lock"),
        br#"version = 3

[[package]]
name = "root_pkg"
version = "0.1.0"
dependencies = [
 "sub_lib",
]
"#,
    )
    .unwrap();

    let manifest_path = root_dir.join("Cargo.toml");
    let opt = RustBuildOptions {
        manifest_path: Some(manifest_path.clone()),
        package: None,
        release: false,
        target_dir: None,
        features: Vec::new(),
        no_default_features: false,
        all_features: false,
        no_cache: false,
        cache_dir: Some(isolated_cache.path().to_path_buf()),
        mode: BuildMode::Build,
        native: true,
        cargo: false,
        extra_args: Vec::new(),
        verbose: 0,
        progress: None,
        ..Default::default()
    };

    let plan = RustAdapter::explain_plan(&opt).unwrap();
    assert!(plan.units.iter().any(|u| u.package_name == "sub_lib"));
}

#[test]
fn test_m8_source_store_overwrite_existing_destination_and_copy() {
    let temp = TempDir::new().unwrap();
    let store_dir = temp.path().join("store");
    let store = SourceStore::new(store_dir).unwrap();

    let crate_bytes = make_tar_gz(&[
        (
            "repl_pkg-1.0.0/Cargo.toml",
            b"[package]\nname = \"repl_pkg\"\nversion = \"1.0.0\"\n",
        ),
        ("repl_pkg-1.0.0/src/lib.rs", b"pub fn v1() {}\n"),
    ]);

    // Pre-create destination directory to test overwriting existing target_dir
    let target = store.package_dir("repl_pkg", "1.0.0");
    fs::create_dir_all(target.join("old_dir")).unwrap();

    let p = store
        .unpack_archive("repl_pkg", "1.0.0", &crate_bytes, "")
        .unwrap();
    assert_eq!(p, target);
    assert!(p.join("Cargo.toml").is_file());
    assert!(!p.join("old_dir").exists());

    // Recursive copy helper
    let copy_src = temp.path().join("copy_src");
    let copy_dst = temp.path().join("copy_dst");
    fs::create_dir_all(copy_src.join("sub")).unwrap();
    fs::write(copy_src.join("sub").join("data.txt"), b"copied_text").unwrap();

    SourceStore::copy_dir_recursive(&copy_src, &copy_dst).unwrap();
    assert!(copy_dst.join("sub").join("data.txt").is_file());
}
