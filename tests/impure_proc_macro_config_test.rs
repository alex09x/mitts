/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Proc macros are pure unless the project declares them in
//! `[workspace.metadata.mitts] impure-proc-macros`. Dependents of a declared macro
//! are keyed on the whole package tree and the absolute path, so an edited data
//! file rebuilds them and another checkout never reuses them. Every other macro's
//! dependents share path-independent keys across checkouts.

use mitts::adapter::rust::key::derive_unit_action_keys;
use mitts::adapter::{RustAdapter, RustBuildOptions};
use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

/// A workspace with a local proc macro and an app that prints its expansion.
/// With `reads_file` the macro embeds `greeting.txt` from the app's package dir
/// without telling rustc about it, exactly what the declaration exists for.
fn write_workspace(root: &Path, mitts_table: &str, reads_file: bool, greeting: &str) {
    fs::create_dir_all(root.join("greeter_macro/src")).unwrap();
    fs::create_dir_all(root.join("greeter_app/src")).unwrap();
    fs::write(
        root.join("Cargo.toml"),
        format!(
            "[workspace]\nmembers = [\"greeter_macro\", \"greeter_app\"]\nresolver = \"2\"\n{mitts_table}"
        ),
    )
    .unwrap();
    fs::write(
        root.join("greeter_macro/Cargo.toml"),
        "[package]\nname = \"greeter_macro\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\nproc-macro = true\n",
    )
    .unwrap();
    let body = if reads_file {
        r#"let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let text = std::fs::read_to_string(std::path::Path::new(&dir).join("greeting.txt")).unwrap();
    format!("{:?}", text.trim()).parse().unwrap()"#
    } else {
        r#""\"hello\"".parse().unwrap()"#
    };
    fs::write(
        root.join("greeter_macro/src/lib.rs"),
        format!(
            "use proc_macro::TokenStream;\n\n#[proc_macro]\npub fn greeting(_input: TokenStream) -> TokenStream {{\n    {body}\n}}\n"
        ),
    )
    .unwrap();
    fs::write(
        root.join("greeter_app/Cargo.toml"),
        "[package]\nname = \"greeter_app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\ngreeter_macro = { path = \"../greeter_macro\" }\n",
    )
    .unwrap();
    fs::write(
        root.join("greeter_app/src/main.rs"),
        "fn main() {\n    print!(\"GREETING:{}\", greeter_macro::greeting!());\n}\n",
    )
    .unwrap();
    fs::write(root.join("greeter_app/greeting.txt"), greeting).unwrap();
}

fn options(root: &Path, cache: &Path) -> RustBuildOptions {
    RustBuildOptions {
        manifest_path: Some(root.join("Cargo.toml")),
        cache_dir: Some(cache.to_path_buf()),
        target_dir: Some(root.join("target")),
        package: Some("greeter_app".to_string()),
        ..Default::default()
    }
}

fn app_key(opt: &RustBuildOptions) -> String {
    let plan = RustAdapter::explain_plan(opt).expect("explain");
    let keys = derive_unit_action_keys(opt, &plan).expect("keys");
    let app = plan
        .units
        .iter()
        .find(|u| u.package_name == "greeter_app" && u.kind == "bin")
        .expect("app unit");
    keys[&app.unit_id()].clone()
}

/// Builds, asserts success, and returns (app restored from cache, program output).
fn build_and_run(root: &Path, opt: &RustBuildOptions) -> (bool, String) {
    let (plan, res) = RustAdapter::build(opt).expect("build");
    assert!(res.success, "build failed:\n{}", res.stderr);
    let restored = plan
        .actions
        .iter()
        .any(|a| a.description.contains("Restored greeter_app"));
    let out = Command::new(root.join("target/debug/greeter_app"))
        .output()
        .expect("run greeter_app");
    (restored, String::from_utf8_lossy(&out.stdout).to_string())
}

#[test]
fn an_undeclared_macro_is_pure_and_its_dependents_are_shared_across_checkouts() {
    let cache = TempDir::new().unwrap();
    let clone1 = TempDir::new().unwrap();
    let clone2 = TempDir::new().unwrap();
    write_workspace(clone1.path(), "", false, "");
    write_workspace(clone2.path(), "", false, "");
    let opt1 = options(clone1.path(), cache.path());
    let opt2 = options(clone2.path(), cache.path());

    assert_eq!(
        app_key(&opt1),
        app_key(&opt2),
        "a crate using an undeclared proc macro must get the same key in every checkout"
    );

    let (restored1, out1) = build_and_run(clone1.path(), &opt1);
    assert!(!restored1, "the first build must compile");
    assert_eq!(out1, "GREETING:hello");

    let (restored2, out2) = build_and_run(clone2.path(), &opt2);
    assert!(
        restored2,
        "the second checkout must restore the app from the cache"
    );
    assert_eq!(out2, "GREETING:hello");
}

#[test]
fn a_declared_macro_keys_its_dependents_on_the_package_files_and_the_path() {
    let cache = TempDir::new().unwrap();
    let clone1 = TempDir::new().unwrap();
    let clone2 = TempDir::new().unwrap();
    let table = "[workspace.metadata.mitts]\nimpure-proc-macros = [\"greeter_macro\"]\n";
    write_workspace(clone1.path(), table, true, "hello");
    write_workspace(clone2.path(), table, true, "hello");
    let opt1 = options(clone1.path(), cache.path());
    let opt2 = options(clone2.path(), cache.path());

    assert_ne!(
        app_key(&opt1),
        app_key(&opt2),
        "a crate using a declared impure macro must be keyed on its own path"
    );

    let (_, out1) = build_and_run(clone1.path(), &opt1);
    assert_eq!(out1, "GREETING:hello");

    // The macro read greeting.txt silently; the package-tree key still sees the edit.
    fs::write(clone1.path().join("greeter_app/greeting.txt"), "bonjour").unwrap();
    let (restored, out) = build_and_run(clone1.path(), &opt1);
    assert!(
        !restored,
        "an edited package file must rebuild the dependents of a declared macro"
    );
    assert_eq!(out, "GREETING:bonjour");

    // Unchanged files in the same checkout hit the cache again.
    let (restored, out) = build_and_run(clone1.path(), &opt1);
    assert!(restored, "the same checkout and files must hit the cache");
    assert_eq!(out, "GREETING:bonjour");

    let (restored2, out2) = build_and_run(clone2.path(), &opt2);
    assert!(
        !restored2,
        "another checkout must not reuse a path-keyed artifact"
    );
    assert_eq!(out2, "GREETING:hello");
}

#[test]
fn a_malformed_declaration_fails_the_build_instead_of_being_ignored() {
    let cache = TempDir::new().unwrap();
    let clone = TempDir::new().unwrap();
    write_workspace(
        clone.path(),
        "[workspace.metadata.mitts]\nimpure-proc-macros = \"greeter_macro\"\n",
        false,
        "",
    );
    let opt = options(clone.path(), cache.path());
    let err = match RustAdapter::build(&opt) {
        Err(e) => e.to_string(),
        Ok((_, res)) => {
            assert!(!res.success, "a malformed declaration must not build");
            res.stderr
        }
    };
    assert!(err.contains("impure-proc-macros"), "{err}");
}

#[test]
fn an_unknown_key_in_the_declaration_warns_on_stderr_and_still_builds() {
    let cache = TempDir::new().unwrap();
    let clone = TempDir::new().unwrap();
    write_workspace(
        clone.path(),
        "[workspace.metadata.mitts]\nimpure-proc-macro = [\"greeter_macro\"]\n",
        false,
        "",
    );
    let out = Command::new(env!("CARGO_BIN_EXE_mitts"))
        .args(["build", "--package", "greeter_app", "--manifest-path"])
        .arg(clone.path().join("Cargo.toml"))
        .arg("--cache-dir")
        .arg(cache.path())
        .env("CI", "true")
        .output()
        .expect("run mitts");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "mitts build failed:\n{stderr}");
    assert!(
        stderr.contains(
            "[mitts] warning: ",
        ) && stderr.contains("[workspace.metadata.mitts] has an unknown key \"impure-proc-macro\" (supported: impure-proc-macros)"),
        "stderr must carry the warning once:\n{stderr}"
    );
    assert_eq!(
        stderr.matches("unknown key \"impure-proc-macro\"").count(),
        1,
        "the warning must be printed once per manifest read, not per unit:\n{stderr}"
    );
}

/// An explicit symlink reproduces Darwin's /var alias on every Unix platform.
/// Base keys and compiled outputs must be reusable between distinct real roots.
#[test]
#[cfg(unix)]
fn a_pure_macro_is_shared_across_symlinked_checkout_roots() {
    let cache = TempDir::new().unwrap();
    let first = TempDir::new().unwrap();
    let second = TempDir::new().unwrap();
    let aliases = TempDir::new().unwrap();
    write_workspace(first.path(), "", false, "");
    write_workspace(second.path(), "", false, "");
    let alias1 = aliases.path().join("first");
    let alias2 = aliases.path().join("second");
    std::os::unix::fs::symlink(first.path(), &alias1).unwrap();
    std::os::unix::fs::symlink(second.path(), &alias2).unwrap();
    let opt1 = options(&alias1, cache.path());
    let opt2 = options(&alias2, cache.path());
    assert_eq!(app_key(&opt1), app_key(&opt2));
    let (restored1, out1) = build_and_run(&alias1, &opt1);
    let (restored2, out2) = build_and_run(&alias2, &opt2);
    assert!(!restored1);
    assert_eq!(out1, "GREETING:hello");
    assert!(
        restored2,
        "canonical source paths must not force ws_leak misses"
    );
    assert_eq!(out2, "GREETING:hello");
    let binary = fs::read(alias2.join("target/debug/greeter_app")).unwrap();
    for path in [
        first.path(),
        second.path(),
        alias1.as_path(),
        alias2.as_path(),
    ] {
        for spelling in [path.to_path_buf(), path.canonicalize().unwrap()] {
            let spelling = spelling.to_string_lossy();
            let needle = spelling.as_bytes();
            assert!(
                !binary.windows(needle.len()).any(|window| window == needle),
                "shared binary retained checkout path {spelling}"
            );
        }
    }
}
