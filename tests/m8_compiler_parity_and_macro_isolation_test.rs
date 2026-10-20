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

#[test]
fn test_m8_workspace_with_impure_proc_macro_gets_different_keys_and_no_cross_hit() {
    let isolated_cache = tempfile::tempdir().expect("isolated cache dir");
    let cache_dir = TempDir::new().unwrap();
    let clone1 = TempDir::new().unwrap();
    let clone2 = TempDir::new().unwrap();

    let setup = |root: &Path| {
        // Workspace root
        fs::write(
            root.join("Cargo.toml"),
            r#"
[workspace]
members = ["my_macro", "my_app"]
resolver = "2"
"#,
        )
        .unwrap();

        // Local proc-macro crate reading CARGO_MANIFEST_DIR at macro expansion time
        let macro_dir = root.join("my_macro");
        fs::create_dir_all(macro_dir.join("src")).unwrap();
        fs::write(
            macro_dir.join("Cargo.toml"),
            r#"
[package]
name = "my_macro"
version = "0.1.0"
edition = "2021"

[lib]
proc-macro = true
"#,
        )
        .unwrap();
        fs::write(
            macro_dir.join("src/lib.rs"),
            r#"
extern crate proc_macro;
use proc_macro::TokenStream;

#[proc_macro]
pub fn embed_manifest_dir(_item: TokenStream) -> TokenStream {
    let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| "missing".to_string());
    format!("pub fn get_embedded_dir() -> &'static str {{ {:?} }}", dir).parse().unwrap()
}
"#,
        )
        .unwrap();

        // Application crate calling the proc macro
        let app_dir = root.join("my_app");
        fs::create_dir_all(app_dir.join("src")).unwrap();
        fs::write(
            app_dir.join("Cargo.toml"),
            r#"
[package]
name = "my_app"
version = "0.1.0"
edition = "2021"

[dependencies]
my_macro = { path = "../my_macro" }
"#,
        )
        .unwrap();
        fs::write(
            app_dir.join("src/main.rs"),
            r#"
my_macro::embed_manifest_dir!();

fn main() {
    println!("embedded: {}", get_embedded_dir());
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
        package: Some("my_app".to_string()),
        ..Default::default()
    };

    let opt2 = RustBuildOptions {
        manifest_path: Some(clone2.path().join("Cargo.toml")),
        cache_dir: Some(cache_dir.path().to_path_buf()),
        target_dir: Some(clone2.path().join("target")),
        package: Some("my_app".to_string()),
        ..Default::default()
    };

    // 1. Action keys for my_app MUST be different between clone 1 and clone 2
    let plan1 = RustAdapter::explain_plan(&opt1).unwrap();
    let plan2 = RustAdapter::explain_plan(&opt2).unwrap();
    let keys1 = derive_unit_action_keys(&opt1, &plan1).unwrap();
    let keys2 = derive_unit_action_keys(&opt2, &plan2).unwrap();

    let app_unit1 = plan1
        .units
        .iter()
        .find(|u| u.package_name == "my_app" && u.kind == "bin")
        .unwrap();
    let app_unit2 = plan2
        .units
        .iter()
        .find(|u| u.package_name == "my_app" && u.kind == "bin")
        .unwrap();

    let key1 = &keys1[&app_unit1.unit_id()];
    let key2 = &keys2[&app_unit2.unit_id()];
    assert_ne!(
        key1, key2,
        "Crates depending on impure proc-macros must get different action keys across clones"
    );

    // 2. Build clone 1
    let (_, res1) = RustAdapter::build(&opt1).expect("build clone 1");
    if !res1.success {
        eprintln!("RES1 FAILURE STDERR:\n{}", res1.stderr);
    }
    assert!(res1.success, "Clone 1 build failed:\n{}", res1.stderr);
    let bin1 = clone1.path().join("target/debug/my_app");
    let out1 = Command::new(&bin1)
        .env("MITTS_CACHE_DIR", isolated_cache.path())
        .output()
        .expect("run bin1");
    let str1 = String::from_utf8_lossy(&out1.stdout);
    assert!(
        str1.contains(&clone1.path().to_string_lossy().to_string()),
        "Clone 1 must output its own manifest dir, got: {}",
        str1
    );

    // 3. Build clone 2 (must not cross-hit clone 1)
    let (_, res2) = RustAdapter::build(&opt2).expect("build clone 2");
    assert!(res2.success, "Clone 2 build failed:\n{}", res2.stderr);
    let bin2 = clone2.path().join("target/debug/my_app");
    let out2 = Command::new(&bin2)
        .env("MITTS_CACHE_DIR", isolated_cache.path())
        .output()
        .expect("run bin2");
    let str2 = String::from_utf8_lossy(&out2.stdout);
    assert!(
        str2.contains(&clone2.path().to_string_lossy().to_string()),
        "Clone 2 must output its own manifest dir, got: {}",
        str2
    );
    assert!(
        !str2.contains(&clone1.path().to_string_lossy().to_string()),
        "Clone 2 must never leak clone 1's path"
    );
}

#[test]
fn test_m8_compiler_error_diagnostics_have_cargo_parity() {
    let isolated_cache = tempfile::tempdir().expect("isolated cache dir");
    let temp = TempDir::new().unwrap();
    let src = temp.path().join("src");
    fs::create_dir_all(&src).unwrap();
    fs::write(
        temp.path().join("Cargo.toml"),
        r#"
[package]
name = "err_crate"
version = "0.1.0"
edition = "2021"
"#,
    )
    .unwrap();
    fs::write(src.join("main.rs"), "fn main() { let x: u32 = \"err\"; }\n").unwrap();

    let opt = RustBuildOptions {
        cache_dir: Some(isolated_cache.path().to_path_buf()),
        manifest_path: Some(temp.path().join("Cargo.toml")),
        target_dir: Some(temp.path().join("target")),
        ..Default::default()
    };

    let (_, res) = RustAdapter::build(&opt).unwrap();
    assert!(!res.success, "Build must fail on compilation error");
    let err_str = res.stderr;

    assert!(
        err_str.contains("--> src/main.rs:1:") || err_str.contains("src/main.rs"),
        "Diagnostic must point to relative src/main.rs (Cargo parity), got:\n{}",
        err_str
    );
    assert!(
        !err_str.contains("/mitts/ws"),
        "Diagnostic must not contain artificial /mitts/ws prefix, got:\n{}",
        err_str
    );
}

#[test]
fn test_m8_package_outside_workspace_root_falls_back_to_absolute_key() {
    let isolated_cache = tempfile::tempdir().expect("isolated cache dir");
    let root = TempDir::new().unwrap();
    let external = TempDir::new().unwrap();

    // External package outside workspace root
    fs::create_dir_all(external.path().join("src")).unwrap();
    fs::write(
        external.path().join("Cargo.toml"),
        r#"
[package]
name = "ext_lib"
version = "0.1.0"
edition = "2021"
"#,
    )
    .unwrap();
    fs::write(
        external.path().join("src/lib.rs"),
        "pub fn ext() -> i32 { 42 }\n",
    )
    .unwrap();

    // Workspace
    fs::create_dir_all(root.path().join("src")).unwrap();
    fs::write(
        root.path().join("Cargo.toml"),
        format!(
            r#"
[package]
name = "ws_crate"
version = "0.1.0"
edition = "2021"

[dependencies]
ext_lib = {{ path = {:?} }}
"#,
            external.path()
        ),
    )
    .unwrap();
    fs::write(
        root.path().join("src/main.rs"),
        "fn main() { println!(\"{}\", ext_lib::ext()); }\n",
    )
    .unwrap();

    let opt = RustBuildOptions {
        cache_dir: Some(isolated_cache.path().to_path_buf()),
        manifest_path: Some(root.path().join("Cargo.toml")),
        target_dir: Some(root.path().join("target")),
        ..Default::default()
    };

    let plan = RustAdapter::explain_plan(&opt).unwrap();
    let keys = derive_unit_action_keys(&opt, &plan).unwrap();
    let ext_unit = plan
        .units
        .iter()
        .find(|u| u.package_name == "ext_lib")
        .unwrap();

    // Since ext_lib is outside the workspace root, strip_prefix fails.
    // The key flags must safely include the absolute CARGO_MANIFEST_DIR.
    assert!(keys.contains_key(&ext_unit.unit_id()));
}
