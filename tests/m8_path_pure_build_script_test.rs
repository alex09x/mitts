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

fn create_pure_build_script_workspace(root: &Path) {
    fs::create_dir_all(root.join("pure_lib/src")).unwrap();

    fs::write(
        root.join("Cargo.toml"),
        r#"[workspace]
members = ["pure_lib"]
resolver = "2"
"#,
    )
    .unwrap();

    fs::write(
        root.join("pure_lib/Cargo.toml"),
        r#"[package]
name = "pure_lib"
version = "0.1.0"
edition = "2021"
"#,
    )
    .unwrap();

    fs::write(
        root.join("pure_lib/build.rs"),
        r##"use std::env;
use std::fs;
use std::path::Path;

fn main() {
    let out_dir = env::var("OUT_DIR").unwrap();
    let dest_path = Path::new(&out_dir).join("hello.rs");
    fs::write(
        &dest_path,
        "pub fn message() -> &'static str {\n    \"hello from pure_lib generated code\"\n}\n",
    )
    .unwrap();
    println!("cargo:rerun-if-changed=build.rs");
}
"##,
    )
    .unwrap();

    fs::write(
        root.join("pure_lib/src/lib.rs"),
        r#"include!(concat!(env!("OUT_DIR"), "/hello.rs"));

pub fn lib_file() -> &'static str {
    file!()
}
"#,
    )
    .unwrap();
}

#[test]
fn test_m8_path_pure_build_script_cache_hit_across_clones() {
    // Test A: Pure build.rs achieves 100% cache hit across distinct clones
    let cache_dir = TempDir::new().unwrap();
    let clone1 = TempDir::new().unwrap();
    let clone2 = TempDir::new().unwrap();

    create_pure_build_script_workspace(clone1.path());
    create_pure_build_script_workspace(clone2.path());

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

    // 1. Action keys must be identical across clones
    let expl1 = RustAdapter::explain_plan(&opt1).expect("explain clone1");
    let expl2 = RustAdapter::explain_plan(&opt2).expect("explain clone2");
    let keys1 = derive_unit_action_keys(&opt1, &expl1).expect("keys clone1");
    let keys2 = derive_unit_action_keys(&opt2, &expl2).expect("keys clone2");

    let lib_unit = expl1
        .units
        .iter()
        .find(|u| u.package_name == "pure_lib")
        .unwrap();

    let lib_key1 = &keys1[&lib_unit.unit_id()];
    let lib_key2 = &keys2[&lib_unit.unit_id()];
    assert_eq!(
        lib_key1, lib_key2,
        "Pure build script library action keys must be path-independent"
    );

    // 2. Clone 1 builds and populates CAS
    let (_, res1) = RustAdapter::build(&opt1).expect("build clone1");
    if !res1.success {
        panic!(
            "build clone1 failed:\nstderr:\n{}\nstdout:\n{}",
            res1.stderr, res1.stdout
        );
    }
    assert!(res1.success);
    assert!(!res1.cached);

    // 2b. Clone 1 rebuilds with target directory cleared: restores build.rs from CAS
    let _ = fs::remove_dir_all(clone1.path().join("target"));
    let (_, res1b) = RustAdapter::build(&opt1).expect("build clone1 second run");
    assert!(res1b.success);
    assert!(
        res1b.cached,
        "Clone 1 second run must hit cache with CAS-restored build.rs"
    );

    // 3. Clone 2 build must hit cache (0 rustc calls)
    let (_, res2) = RustAdapter::build(&opt2).expect("build clone2");
    assert!(res2.success);
    assert!(
        res2.cached,
        "Clone 2 build must hit cache for pure build script crate"
    );

    // 4. Verify artifact contains NO bytes of clone 1 path
    let rlib2 = clone2.path().join("target/debug/deps/libpure_lib.rlib");
    assert!(rlib2.is_file(), "rlib must exist in clone 2 target dir");
    let rlib2_bytes = fs::read(&rlib2).expect("read rlib bytes");
    let clone1_path_bytes = clone1.path().to_string_lossy().as_bytes().to_vec();
    assert!(
        !rlib2_bytes
            .windows(clone1_path_bytes.len())
            .any(|w| w == clone1_path_bytes.as_slice()),
        "Restored rlib must not contain clone 1's path"
    );
}

#[test]
fn test_m8_impure_build_script_safe_fallback_no_cross_worktree_hit() {
    // Test B: Impure build script (leaking absolute path into directive) does NOT hit across clones
    let cache_dir = TempDir::new().unwrap();
    let clone1 = TempDir::new().unwrap();
    let clone2 = TempDir::new().unwrap();

    let setup = |root: &Path| {
        fs::create_dir_all(root.join("impure_lib/src")).unwrap();
        fs::write(
            root.join("Cargo.toml"),
            r#"[workspace]
members = ["impure_lib"]
resolver = "2"
"#,
        )
        .unwrap();
        fs::write(
            root.join("impure_lib/Cargo.toml"),
            r#"[package]
name = "impure_lib"
version = "0.1.0"
edition = "2021"
"#,
        )
        .unwrap();
        fs::write(
            root.join("impure_lib/build.rs"),
            r#"use std::env;

fn main() {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    // Leaks absolute path into compiler flag/directive
    println!("cargo:rustc-env=LEAKED_PATH={}", manifest_dir);
}
"#,
        )
        .unwrap();
        fs::write(
            root.join("impure_lib/src/lib.rs"),
            r#"pub fn leaked_path() -> &'static str {
    env!("LEAKED_PATH")
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
        ..Default::default()
    };

    let opt2 = RustBuildOptions {
        manifest_path: Some(clone2.path().join("Cargo.toml")),
        cache_dir: Some(cache_dir.path().to_path_buf()),
        ..Default::default()
    };

    // 1. Clone 1 builds and compiles into CAS
    let (_, res1) = RustAdapter::build(&opt1).expect("build clone1");
    assert!(res1.success);
    assert!(!res1.cached);

    // 2. Clone 2 MUST NOT hit cache (safe fallback, isolated by ws_hash/impurity)
    let (_, res2) = RustAdapter::build(&opt2).expect("build clone2");
    assert!(res2.success);
    assert!(
        !res2.cached,
        "Clone 2 build must not hit cache when build script leaks absolute paths"
    );
}

#[test]
fn test_m8_dependent_crate_on_pure_build_script_crate_hits_across_clones() {
    // Test C: Dependent crate depending on pure build.rs crate achieves 100% cache hit in clone 2
    let cache_dir = TempDir::new().unwrap();
    let clone1 = TempDir::new().unwrap();
    let clone2 = TempDir::new().unwrap();

    let setup = |root: &Path| {
        create_pure_build_script_workspace(root);
        fs::create_dir_all(root.join("pure_app/src")).unwrap();

        fs::write(
            root.join("Cargo.toml"),
            r#"[workspace]
members = ["pure_lib", "pure_app"]
resolver = "2"
"#,
        )
        .unwrap();

        fs::write(
            root.join("pure_app/Cargo.toml"),
            r#"[package]
name = "pure_app"
version = "0.1.0"
edition = "2021"

[dependencies]
pure_lib = { path = "../pure_lib" }
"#,
        )
        .unwrap();

        fs::write(
            root.join("pure_app/src/main.rs"),
            r#"fn main() {
    print!("{}", pure_lib::message());
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

    // 1. Clone 1 cold build compiles both pure_lib and pure_app
    let (_, res1) = RustAdapter::build(&opt1).expect("build clone1");
    assert!(res1.success);
    assert!(!res1.cached);

    // 2. Clone 2 build must be 100% cache hit for BOTH pure_lib and pure_app
    let (_, res2) = RustAdapter::build(&opt2).expect("build clone2");
    assert!(res2.success);
    assert!(
        res2.cached,
        "Clone 2 build must hit cache for both pure build.rs crate and its dependent"
    );

    // 3. Executable runs successfully and output matches
    let app2 = clone2.path().join("target/debug/pure_app");
    assert!(app2.is_file(), "pure_app binary must exist in clone 2");
    let output = Command::new(&app2).output().expect("run app2");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout, "hello from pure_lib generated code");
}

#[test]
fn test_m8_cc_native_build_script_cache_hit_across_clones() {
    // Test D: cc / native C style build.rs emitting rustc-link-search=native=$OUT_DIR and
    // rustc-link-lib=static=foo under custom HOME achieves 100% hit across distinct clones
    let temp_home = TempDir::new().unwrap();
    let cache_dir = temp_home.path().join(".cache/mitts");
    let clone1 = TempDir::new().unwrap();
    let clone2 = TempDir::new().unwrap();

    let setup = |root: &Path| {
        fs::create_dir_all(root.join("c_lib/src")).unwrap();
        fs::write(
            root.join("Cargo.toml"),
            r#"[workspace]
members = ["c_lib"]
resolver = "2"
"#,
        )
        .unwrap();

        fs::write(
            root.join("c_lib/Cargo.toml"),
            r#"[package]
name = "c_lib"
version = "0.1.0"
edition = "2021"
"#,
        )
        .unwrap();

        fs::write(
            root.join("c_lib/build.rs"),
            r##"use std::env;
use std::fs;
use std::path::Path;

fn main() {
    let out_dir = env::var("OUT_DIR").unwrap();
    let lib_path = Path::new(&out_dir).join("libfoo.a");
    // Write valid ar archive header for static library
    fs::write(&lib_path, b"!<arch>\n").unwrap();
    println!("cargo:rustc-link-search=native={}", out_dir);
    println!("cargo:rustc-link-lib=static=foo");
    println!("cargo:rerun-if-changed=build.rs");
}
"##,
        )
        .unwrap();

        fs::write(
            root.join("c_lib/src/lib.rs"),
            r#"pub fn dummy() -> i32 {
    42
}
"#,
        )
        .unwrap();
    };

    setup(clone1.path());
    setup(clone2.path());

    let prev_home = std::env::var("HOME").ok();
    std::env::set_var("HOME", temp_home.path());

    let opt1 = RustBuildOptions {
        manifest_path: Some(clone1.path().join("Cargo.toml")),
        cache_dir: Some(cache_dir.clone()),
        target_dir: Some(clone1.path().join("target")),
        ..Default::default()
    };

    let opt2 = RustBuildOptions {
        manifest_path: Some(clone2.path().join("Cargo.toml")),
        cache_dir: Some(cache_dir.clone()),
        target_dir: Some(clone2.path().join("target")),
        ..Default::default()
    };

    let expl1 = RustAdapter::explain_plan(&opt1).expect("explain clone1");
    let expl2 = RustAdapter::explain_plan(&opt2).expect("explain clone2");
    let keys1 = derive_unit_action_keys(&opt1, &expl1).expect("keys clone1");
    let keys2 = derive_unit_action_keys(&opt2, &expl2).expect("keys clone2");
    assert_eq!(
        keys1, keys2,
        "Action keys for cc-native crate must be identical across clones"
    );

    let (_, res1) = RustAdapter::build(&opt1).expect("build clone1");
    assert!(res1.success);
    assert!(!res1.cached);

    let (_, res2) = RustAdapter::build(&opt2).expect("build clone2");
    if let Some(ref h) = prev_home {
        std::env::set_var("HOME", h);
    }
    assert!(res2.success);
    assert!(
        res2.cached,
        "Clone 2 build must hit cache for cc-style build script crate with native link-search"
    );
}

#[test]
fn test_m8_external_dependency_shared_across_workspaces() {
    // Test E: External dependency outside workspace roots is shared across two distinct workspaces
    let temp_root = TempDir::new().unwrap();
    let cache_dir = temp_root.path().join("cache");
    let ext_dir = temp_root.path().join("external_dep");
    let ws1 = temp_root.path().join("ws1");
    let ws2 = temp_root.path().join("ws2");

    fs::create_dir_all(ext_dir.join("src")).unwrap();
    fs::write(
        ext_dir.join("Cargo.toml"),
        r#"[package]
name = "external_dep"
version = "0.1.0"
edition = "2021"
"#,
    )
    .unwrap();
    fs::write(
        ext_dir.join("build.rs"),
        r##"use std::env;
use std::fs;
use std::path::Path;

fn main() {
    let out_dir = env::var("OUT_DIR").unwrap();
    let lib_path = Path::new(&out_dir).join("libbar.a");
    fs::write(&lib_path, b"!<arch>\n").unwrap();
    println!("cargo:rustc-link-search=native={}", out_dir);
    println!("cargo:rustc-link-lib=static=bar");
    println!("cargo:rerun-if-changed=build.rs");
}
"##,
    )
    .unwrap();
    fs::write(
        ext_dir.join("src/lib.rs"),
        r#"pub fn ext_func() -> &'static str {
    "hello from external"
}
"#,
    )
    .unwrap();

    let setup_ws = |ws_dir: &Path| {
        fs::create_dir_all(ws_dir.join("app/src")).unwrap();
        fs::write(
            ws_dir.join("Cargo.toml"),
            r#"[workspace]
members = ["app"]
resolver = "2"
"#,
        )
        .unwrap();

        fs::write(
            ws_dir.join("app/Cargo.toml"),
            format!(
                r#"[package]
name = "app"
version = "0.1.0"
edition = "2021"

[dependencies]
external_dep = {{ path = "{}" }}
"#,
                ext_dir.display()
            ),
        )
        .unwrap();

        fs::write(
            ws_dir.join("app/src/main.rs"),
            r#"fn main() {
    println!("{}", external_dep::ext_func());
}
"#,
        )
        .unwrap();
    };

    setup_ws(&ws1);
    setup_ws(&ws2);

    let opt1 = RustBuildOptions {
        manifest_path: Some(ws1.join("Cargo.toml")),
        cache_dir: Some(cache_dir.clone()),
        target_dir: Some(ws1.join("target")),
        ..Default::default()
    };

    let opt2 = RustBuildOptions {
        manifest_path: Some(ws2.join("Cargo.toml")),
        cache_dir: Some(cache_dir.clone()),
        target_dir: Some(ws2.join("target")),
        ..Default::default()
    };

    let (_, res1) = RustAdapter::build(&opt1).expect("build ws1");
    assert!(res1.success);
    assert!(!res1.cached);

    let (_, res2) = RustAdapter::build(&opt2).expect("build ws2");
    assert!(res2.success);
    assert!(
        res2.cached,
        "External dependency must be reusable across workspaces"
    );
}

#[test]
fn test_m8_impure_build_script_hits_in_same_workspace() {
    // Test F: Impure build script hits cache on second run in the SAME workspace
    let cache_dir = TempDir::new().unwrap();
    let ws = TempDir::new().unwrap();

    fs::create_dir_all(ws.path().join("impure_crate/src")).unwrap();
    fs::write(
        ws.path().join("Cargo.toml"),
        r#"[workspace]
members = ["impure_crate"]
resolver = "2"
"#,
    )
    .unwrap();

    fs::write(
        ws.path().join("impure_crate/Cargo.toml"),
        r#"[package]
name = "impure_crate"
version = "0.1.0"
edition = "2021"
"#,
    )
    .unwrap();

    fs::write(
        ws.path().join("impure_crate/build.rs"),
        r##"use std::env;

fn main() {
    let cwd = env::current_dir().unwrap();
    // Leaks workspace path into rustc-cfg, making it impure
    println!("cargo:rustc-cfg=leaked_path=\"{}\"", cwd.display());
    println!("cargo:rerun-if-changed=build.rs");
}
"##,
    )
    .unwrap();

    fs::write(
        ws.path().join("impure_crate/src/lib.rs"),
        r#"pub fn check() -> bool {
    true
}
"#,
    )
    .unwrap();

    let opt = RustBuildOptions {
        manifest_path: Some(ws.path().join("Cargo.toml")),
        cache_dir: Some(cache_dir.path().to_path_buf()),
        target_dir: Some(ws.path().join("target")),
        ..Default::default()
    };

    // 1. First run compiles cold
    let (_, res1) = RustAdapter::build(&opt).expect("build run 1");
    assert!(res1.success);
    assert!(!res1.cached);

    // 2. Clear local target directory
    let _ = fs::remove_dir_all(ws.path().join("target"));

    // 3. Second run in the same workspace MUST hit cache
    let (_, res2) = RustAdapter::build(&opt).expect("build run 2");
    assert!(res2.success);
    assert!(
        res2.cached,
        "Impure build script must hit cache on second run in the same workspace"
    );
}
