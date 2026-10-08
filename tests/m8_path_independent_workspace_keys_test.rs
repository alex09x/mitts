/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::rust::key::derive_unit_action_keys;
use mitts::adapter::{RustAdapter, RustBuildOptions};
use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

fn create_two_crate_workspace(root: &Path) {
    fs::create_dir_all(root.join("my_lib/src")).unwrap();
    fs::create_dir_all(root.join("my_bin/src")).unwrap();

    fs::write(
        root.join("Cargo.toml"),
        r#"[workspace]
members = ["my_lib", "my_bin"]
resolver = "2"
"#,
    )
    .unwrap();

    fs::write(
        root.join("my_lib/Cargo.toml"),
        r#"[package]
name = "my_lib"
version = "0.1.0"
edition = "2021"
"#,
    )
    .unwrap();

    fs::write(
        root.join("my_lib/src/lib.rs"),
        r#"pub fn message() -> &'static str {
    "hello from my_lib"
}

pub fn lib_file() -> &'static str {
    file!()
}
"#,
    )
    .unwrap();

    fs::write(
        root.join("my_bin/Cargo.toml"),
        r#"[package]
name = "my_bin"
version = "0.1.0"
edition = "2021"

[dependencies]
my_lib = { path = "../my_lib" }
"#,
    )
    .unwrap();

    fs::write(
        root.join("my_bin/src/main.rs"),
        r#"fn main() {
    println!("{}: {}", my_lib::message(), file!());
}
"#,
    )
    .unwrap();
}

#[test]
fn test_m8_workspace_path_independent_keys_and_cas_reuse() {
    let cache_dir = TempDir::new().unwrap();
    let clone1 = TempDir::new().unwrap();
    let clone2 = TempDir::new().unwrap();

    create_two_crate_workspace(clone1.path());
    create_two_crate_workspace(clone2.path());

    let opt1 = RustBuildOptions {
        manifest_path: Some(clone1.path().join("Cargo.toml")),
        cache_dir: Some(cache_dir.path().to_path_buf()),
        target_dir: Some(clone1.path().join("target")),
        ..Default::default()
    };

    let opt2 = RustBuildOptions {
        manifest_path: Some(clone2.path().join("Cargo.toml")),
        cache_dir: Some(cache_dir.path().to_path_buf()),
        target_dir: Some(clone2.path().join("target")),
        ..Default::default()
    };

    // 1. Action keys for lib and bin must be identical across both clones
    let expl1 = RustAdapter::explain_plan(&opt1).expect("explain clone1");
    let expl2 = RustAdapter::explain_plan(&opt2).expect("explain clone2");
    let keys1 = derive_unit_action_keys(&opt1, &expl1).expect("keys clone1");
    let keys2 = derive_unit_action_keys(&opt2, &expl2).expect("keys clone2");

    let lib_unit = expl1
        .units
        .iter()
        .find(|u| u.package_name == "my_lib")
        .unwrap();
    let bin_unit = expl1
        .units
        .iter()
        .find(|u| u.package_name == "my_bin")
        .unwrap();

    let lib_key1 = &keys1[&lib_unit.unit_id()];
    let lib_key2 = &keys2[&lib_unit.unit_id()];
    let bin_key1 = &keys1[&bin_unit.unit_id()];
    let bin_key2 = &keys2[&bin_unit.unit_id()];

    assert_eq!(
        lib_key1, lib_key2,
        "Library action keys must be path-independent across clones"
    );
    assert_eq!(
        bin_key1, bin_key2,
        "Binary action keys must be path-independent across clones"
    );

    // 2. Clone 1 cold build compiles and populates CAS
    let (_, res1) = RustAdapter::build(&opt1).expect("build clone1");
    assert!(res1.success);
    assert!(!res1.cached);

    // 3. Clone 2 build must be 100% cache hit (0 rustc calls)
    let (_, res2) = RustAdapter::build(&opt2).expect("build clone2");
    assert!(res2.success);
    assert!(
        res2.cached,
        "Clone 2 build must hit cache for path-independent workspace crates"
    );

    // 4. Binary in clone 2 must run successfully and print expected output
    let bin2_path = clone2.path().join("target/debug/my_bin");
    assert!(bin2_path.is_file(), "Binary must be restored in clone 2");

    let output = Command::new(&bin2_path).output().expect("run bin2");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("hello from my_lib"),
        "Output must contain library message"
    );
    assert!(
        stdout.contains("/mitts/ws/my_bin/src/main.rs"),
        "file!() must reflect stable remapped prefix"
    );

    // 5. Binary bytes in clone 2 must contain NO occurrences of clone 1 path
    let bin2_bytes = fs::read(&bin2_path).expect("read bin2 bytes");
    let clone1_path_bytes = clone1.path().to_string_lossy().as_bytes().to_vec();
    assert!(
        !bin2_bytes
            .windows(clone1_path_bytes.len())
            .any(|w| w == clone1_path_bytes.as_slice()),
        "Binary restored in clone 2 must not contain clone 1's path"
    );
}

#[test]
fn test_m8_workspace_crate_with_env_manifest_dir_has_different_keys() {
    let cache_dir = TempDir::new().unwrap();
    let clone1 = TempDir::new().unwrap();
    let clone2 = TempDir::new().unwrap();

    let setup = |root: &Path| {
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(
            root.join("Cargo.toml"),
            r#"[package]
name = "env_crate"
version = "0.1.0"
edition = "2021"
"#,
        )
        .unwrap();
        fs::write(
            root.join("src/main.rs"),
            r#"fn main() {
    let dir = env!("CARGO_MANIFEST_DIR");
    println!("MANIFEST:{}", dir);
}
"#,
        )
        .unwrap();
    };

    setup(clone1.path());
    setup(clone2.path());

    let opt1 = RustBuildOptions {
        manifest_path: Some(clone1.path().join("Cargo.toml")),
        cache_dir: Some(cache_dir.path().to_path_buf()),
        target_dir: Some(clone1.path().join("target")),
        ..Default::default()
    };

    let opt2 = RustBuildOptions {
        manifest_path: Some(clone2.path().join("Cargo.toml")),
        cache_dir: Some(cache_dir.path().to_path_buf()),
        target_dir: Some(clone2.path().join("target")),
        ..Default::default()
    };

    let expl1 = RustAdapter::explain_plan(&opt1).expect("explain 1");
    let expl2 = RustAdapter::explain_plan(&opt2).expect("explain 2");
    let keys1 = derive_unit_action_keys(&opt1, &expl1).expect("keys 1");
    let keys2 = derive_unit_action_keys(&opt2, &expl2).expect("keys 2");

    let unit_id = expl1.units[0].unit_id();
    assert_ne!(
        keys1[&unit_id], keys2[&unit_id],
        "Crates reading CARGO_MANIFEST_DIR must produce distinct action keys"
    );

    // Build and execute in clone 2: must print clone 2's path
    let (_, res1) = RustAdapter::build(&opt1).expect("build 1");
    assert!(res1.success);

    let (_, res2) = RustAdapter::build(&opt2).expect("build 2");
    assert!(res2.success);

    let bin2 = clone2.path().join("target/debug/env_crate");
    let output = Command::new(&bin2).output().expect("run env_crate");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(&format!("MANIFEST:{}", clone2.path().display())),
        "Binary must print clone 2 manifest dir, got: {}",
        stdout
    );
}

#[test]
fn test_m8_workspace_crate_with_build_script_has_different_keys() {
    let cache_dir = TempDir::new().unwrap();
    let clone1 = TempDir::new().unwrap();
    let clone2 = TempDir::new().unwrap();

    let setup = |root: &Path| {
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(
            root.join("Cargo.toml"),
            r#"[package]
name = "build_rs_crate"
version = "0.1.0"
edition = "2021"
"#,
        )
        .unwrap();
        fs::write(root.join("build.rs"), "fn main() {}\n").unwrap();
        fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
    };

    setup(clone1.path());
    setup(clone2.path());

    let opt1 = RustBuildOptions {
        manifest_path: Some(clone1.path().join("Cargo.toml")),
        cache_dir: Some(cache_dir.path().to_path_buf()),
        target_dir: Some(clone1.path().join("target")),
        ..Default::default()
    };

    let opt2 = RustBuildOptions {
        manifest_path: Some(clone2.path().join("Cargo.toml")),
        cache_dir: Some(cache_dir.path().to_path_buf()),
        target_dir: Some(clone2.path().join("target")),
        ..Default::default()
    };

    let expl1 = RustAdapter::explain_plan(&opt1).expect("explain 1");
    let expl2 = RustAdapter::explain_plan(&opt2).expect("explain 2");
    let keys1 = derive_unit_action_keys(&opt1, &expl1).expect("keys 1");
    let keys2 = derive_unit_action_keys(&opt2, &expl2).expect("keys 2");

    let unit_id = expl1.units[0].unit_id();
    assert_ne!(
        keys1[&unit_id], keys2[&unit_id],
        "Crates with build.rs must conservatively remain per-worktree with distinct action keys"
    );
}

#[test]
fn test_m8_panic_locations_point_to_stable_prefix() {
    let cache_dir = TempDir::new().unwrap();
    let clone1 = TempDir::new().unwrap();
    let clone2 = TempDir::new().unwrap();

    let setup = |root: &Path| {
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(
            root.join("Cargo.toml"),
            r#"[package]
name = "panic_crate"
version = "0.1.0"
edition = "2021"
"#,
        )
        .unwrap();
        fs::write(
            root.join("src/main.rs"),
            r#"fn main() {
    panic!("intentional_panic_for_location_check");
}
"#,
        )
        .unwrap();
    };

    setup(clone1.path());
    setup(clone2.path());

    let opt1 = RustBuildOptions {
        manifest_path: Some(clone1.path().join("Cargo.toml")),
        cache_dir: Some(cache_dir.path().to_path_buf()),
        target_dir: Some(clone1.path().join("target")),
        ..Default::default()
    };

    let opt2 = RustBuildOptions {
        manifest_path: Some(clone2.path().join("Cargo.toml")),
        cache_dir: Some(cache_dir.path().to_path_buf()),
        target_dir: Some(clone2.path().join("target")),
        ..Default::default()
    };

    let (_, res1) = RustAdapter::build(&opt1).expect("build 1");
    assert!(res1.success);

    let (_, res2) = RustAdapter::build(&opt2).expect("build 2");
    assert!(res2.success);
    assert!(res2.cached, "Clone 2 should reuse binary from CAS");

    let bin2 = clone2.path().join("target/debug/panic_crate");
    let output = Command::new(&bin2).output().expect("run panic_crate");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        stderr.contains("/mitts/ws/src/main.rs"),
        "Panic location must point to remapped stable prefix (/mitts/ws/src/main.rs), got: {}",
        stderr
    );
    assert!(
        !stderr.contains(&clone1.path().to_string_lossy().to_string()),
        "Panic stderr must not leak clone 1's path"
    );
}
