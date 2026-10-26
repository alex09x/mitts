/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! The `env!` scan of a local package reads a file's bytes only when its length or mtime
//! changed; added, edited, removed and dynamic files still change the answer.

#![cfg(unix)]

use mitts::adapter::rust::env_macro::discover_package_env_macros;
use mitts::adapter::rust::toolchain_probes::set_cache_root;
use mitts::adapter::rust::types::EnvMacroDiscovery;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::{Duration, SystemTime};
use tempfile::TempDir;

fn write(path: &Path, body: &str) {
    fs::write(path, body).unwrap();
    let later = SystemTime::now() + Duration::from_secs(10);
    fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(later)
        .unwrap();
}

fn vars(pkg: &Path) -> EnvMacroDiscovery {
    discover_package_env_macros(pkg).0
}

#[test]
fn env_macro_scan_reads_only_changed_files() {
    let cache = TempDir::new().unwrap();
    set_cache_root(cache.path());
    let temp = TempDir::new().unwrap();
    let pkg = temp.path().join("pkg");
    fs::create_dir_all(pkg.join("src")).unwrap();
    write(
        &pkg.join("src/lib.rs"),
        "pub const A: &str = env!(\"A\");\n",
    );
    write(
        &pkg.join("src/b.rs"),
        "pub fn b() -> Option<&'static str> { option_env!(\"B\") }\n",
    );
    let tracked =
        |names: &[&str]| EnvMacroDiscovery::Tracked(names.iter().map(|s| s.to_string()).collect());

    assert_eq!(vars(&pkg), tracked(&["A", "B"]));
    let memo_dir = cache.path().join("env_macros");
    assert_eq!(
        fs::read_dir(&memo_dir).unwrap().count(),
        1,
        "one memo per package"
    );

    // Unchanged files are not read: an unreadable file still answers from the memo.
    fs::set_permissions(pkg.join("src/lib.rs"), fs::Permissions::from_mode(0o000)).unwrap();
    assert_eq!(vars(&pkg), tracked(&["A", "B"]));
    fs::set_permissions(pkg.join("src/lib.rs"), fs::Permissions::from_mode(0o644)).unwrap();

    // Edited, added and removed files change the answer.
    write(
        &pkg.join("src/b.rs"),
        "pub fn b() -> &'static str { env!(\"C\") }\n",
    );
    assert_eq!(vars(&pkg), tracked(&["A", "C"]));
    write(&pkg.join("src/d.rs"), "pub const D: &str = env!(\"D\");\n");
    assert_eq!(vars(&pkg), tracked(&["A", "C", "D"]));
    fs::remove_file(pkg.join("src/b.rs")).unwrap();
    assert_eq!(vars(&pkg), tracked(&["A", "D"]));

    // A dynamic argument poisons the package until it is gone again.
    write(
        &pkg.join("src/e.rs"),
        "pub fn e() -> &'static str { env!(NAME) }\n",
    );
    assert_eq!(vars(&pkg), EnvMacroDiscovery::UntrackedDynamic);
    assert_eq!(vars(&pkg), EnvMacroDiscovery::UntrackedDynamic);
    fs::remove_file(pkg.join("src/e.rs")).unwrap();
    assert_eq!(vars(&pkg), tracked(&["A", "D"]));

    // A nested package's files belong to that package, not to this one.
    fs::create_dir_all(pkg.join("member/src")).unwrap();
    write(
        &pkg.join("member/Cargo.toml"),
        "[package]\nname = \"member\"\n",
    );
    write(
        &pkg.join("member/src/lib.rs"),
        "pub const M: &str = env!(\"MEMBER_ONLY\");\n",
    );
    assert_eq!(vars(&pkg), tracked(&["A", "D"]));
    assert_eq!(vars(&pkg.join("member")), tracked(&["MEMBER_ONLY"]));

    // A damaged memo is ignored, not trusted.
    for entry in fs::read_dir(&memo_dir).unwrap().flatten() {
        fs::write(entry.path(), b"{ not json").unwrap();
    }
    assert_eq!(vars(&pkg), tracked(&["A", "D"]));
    assert_eq!(vars(&pkg), tracked(&["A", "D"]));
}
