/*
 * Mitts Build — a dependent restored from the shared cache must match its dependency's bytes
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Regression for issue 98. The action cache is last-writer-wins: when two builds publish
//! different bytes for one dependency key (concurrent workspaces, rustc nondeterminism),
//! a dependent cached against the first build used to be restored next to the second one,
//! and every later build failed with E0460 ("found possibly newer version of crate") until
//! the cache was wiped. Dependents now key on the dependency's `.rmeta` bytes.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn write(root: &Path, rel: &str, body: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

fn build(root: &Path, cache: &Path, target: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mitts"))
        .current_dir(root)
        .args(["build", "--offline", "-p", "top", "--cache-dir"])
        .arg(cache)
        .arg("--target-dir")
        .arg(target)
        .env_remove("MITTS_CACHE_DIR")
        .output()
        .unwrap()
}

fn assert_ok(label: &str, out: &Output) {
    assert!(
        out.status.success(),
        "{label} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

fn package(name: &str, deps: &str) -> String {
    format!("[package]\nname='{name}'\nversion='0.1.0'\nedition='2021'\n[dependencies]\n{deps}")
}

/// Action-cache entries (JSON files) whose `action_id` names the library of `crate_name`.
fn lib_entries(cache: &Path, crate_name: &str) -> BTreeSet<PathBuf> {
    let mut found = BTreeSet::new();
    let mut stack = vec![cache.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in rd.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
                continue;
            };
            let id = json["action_id"].as_str().unwrap_or_default();
            // Unit ids read `<package id>::<kind>:<target name>`.
            let is_lib = id.rsplit_once("::").is_some_and(|(_, tail)| {
                tail.contains("lib") && tail.ends_with(&format!(":{crate_name}"))
            });
            let has_rlib = json["outputs"].as_array().is_some_and(|outs| {
                outs.iter().any(|o| {
                    o["relative_path"]
                        .as_str()
                        .is_some_and(|p| p.ends_with(".rlib"))
                })
            });
            if is_lib && has_rlib {
                found.insert(path);
            }
        }
    }
    found
}

#[test]
fn dependent_is_rebuilt_when_its_dependency_entry_is_overwritten_with_other_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("ws");
    let cache = temp.path().join("cache");
    write(
        &root,
        "Cargo.toml",
        "[workspace]\nresolver='2'\nmembers=['dep','mid','top']\n",
    );
    write(&root, "dep/Cargo.toml", &package("dep", ""));
    let dep_x = "pub fn value() -> u32 { 1 }\n";
    let dep_y = "pub fn value() -> u32 { 2 }\npub struct Other;\n";
    write(&root, "dep/src/lib.rs", dep_x);
    write(
        &root,
        "mid/Cargo.toml",
        &package("mid", "dep={path='../dep'}\n"),
    );
    write(
        &root,
        "mid/src/lib.rs",
        "pub fn twice() -> u32 { dep::value() * 2 }\n",
    );
    write(
        &root,
        "top/Cargo.toml",
        &package("top", "mid={path='../mid'}\n"),
    );
    write(
        &root,
        "top/src/lib.rs",
        "pub fn top() -> u32 { mid::twice() }\n",
    );

    // 1. Build with dep = X: publishes dep(X) and mid compiled against dep(X).
    assert_ok("build X", &build(&root, &cache, &temp.path().join("t1")));
    let entries_x = lib_entries(&cache, "dep");
    assert_eq!(entries_x.len(), 1, "one dep entry after the first build");
    let entry_x = entries_x.iter().next().unwrap().clone();

    // 2. Build with dep = Y to get a second, different set of dep bytes into the store.
    write(&root, "dep/src/lib.rs", dep_y);
    assert_ok("build Y", &build(&root, &cache, &temp.path().join("t2")));
    let entries_y: Vec<_> = lib_entries(&cache, "dep")
        .difference(&entries_x)
        .cloned()
        .collect();
    assert_eq!(entries_y.len(), 1, "a second dep entry after the Y build");

    // 3. Simulate the lost race: dep's X key now holds the Y bytes, as a later writer
    //    of the same key would leave it. mid's X entry still says "built against X".
    let x_json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&entry_x).unwrap()).unwrap();
    let mut y_json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&entries_y[0]).unwrap()).unwrap();
    y_json["action_key"] = x_json["action_key"].clone();
    std::fs::write(&entry_x, serde_json::to_vec_pretty(&y_json).unwrap()).unwrap();

    // 4. Back to X, touch only `top`, build into a fresh target dir (as after `mitts clean`).
    //    dep restores the overwritten entry; mid must not be restored against it.
    write(&root, "dep/src/lib.rs", dep_x);
    write(
        &root,
        "top/src/lib.rs",
        "pub fn top() -> u32 { mid::twice() + 1 }\n",
    );
    let out = build(&root, &cache, &temp.path().join("t3"));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("E0460") && !stderr.contains("E0463"),
        "inconsistent dependency pair handed to rustc:\n{stderr}"
    );
    assert_ok("build after overwrite", &out);
}
