/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A build-script cache entry whose `meta.toml` is gone can never be restored. It is
//! what an interrupted writer or a plain `rm -rf` leaves behind: the directives and
//! `.last_used` are writable and go first, the read-only output tree survives. The
//! next execution must replace such an entry instead of skipping the save forever.

use mitts::adapter::rust::build_script::execute_build_script;
use mitts::adapter::rust::types::PlannedCompilationUnit;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn write_package(pkg_dir: &Path) -> PathBuf {
    fs::create_dir_all(pkg_dir.join("src")).unwrap();
    let manifest = pkg_dir.join("Cargo.toml");
    fs::write(
        &manifest,
        "[package]\nname = \"gen_pkg\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(pkg_dir.join("src/lib.rs"), "").unwrap();
    fs::write(
        pkg_dir.join("build.rs"),
        r#"fn main() {
    let out_dir = std::env::var("OUT_DIR").unwrap();
    std::fs::write(std::path::Path::new(&out_dir).join("generated.rs"), "pub const N: u32 = 7;").unwrap();
}
"#,
    )
    .unwrap();
    manifest
}

fn entry_dirs(cas_dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for key in fs::read_dir(cas_dir.join("build_scripts"))
        .unwrap()
        .flatten()
    {
        for entry in fs::read_dir(key.path()).unwrap().flatten() {
            if entry.path().is_dir() && !entry.file_name().to_string_lossy().starts_with('.') {
                out.push(entry.path());
            }
        }
    }
    out
}

#[test]
fn an_entry_that_lost_its_directives_is_replaced_on_the_next_execution() {
    let temp = TempDir::new().unwrap();
    let pkg_dir = temp.path().join("gen_pkg");
    let manifest = write_package(&pkg_dir);
    let cas_dir = temp.path().join("cas");
    let artifacts_dir = cas_dir.join("artifacts");
    fs::create_dir_all(&artifacts_dir).unwrap();
    let unit = PlannedCompilationUnit {
        package_id: "gen_pkg@0.1.0".to_string(),
        package_name: "gen_pkg".to_string(),
        version: "0.1.0".to_string(),
        target_name: "gen_pkg".to_string(),
        kind: "lib".to_string(),
        src_path: pkg_dir.join("src/lib.rs"),
        manifest_path: manifest,
        edition: Some("2021".to_string()),
        is_workspace: true,
        cache_eligibility: "cacheable".to_string(),
        ..Default::default()
    };
    let run = |n: u32| {
        let staging = temp.path().join(format!("staging_{n}"));
        fs::create_dir_all(&staging).unwrap();
        execute_build_script(&unit, &staging, false, &[], Some(&artifacts_dir), &[], None)
            .unwrap_or_else(|e| panic!("run {n} failed: {e}"))
    };

    let d1 = run(1);
    let entries = entry_dirs(&cas_dir);
    assert_eq!(entries.len(), 1, "{entries:?}");
    let entry = entries[0].clone();
    assert!(entry.join("meta.toml").is_file());
    let cached_out = d1.out_dir.clone().unwrap();
    assert!(
        cached_out.starts_with(&entry),
        "{cached_out:?} not under {entry:?}"
    );
    assert!(cached_out.join("generated.rs").is_file());

    // What `rm -rf` leaves of a read-only entry: the writable files are gone, the output stays.
    fs::remove_file(entry.join("meta.toml")).unwrap();
    let _ = fs::remove_file(entry.join(".last_used"));
    assert!(cached_out.join("generated.rs").is_file());

    let d2 = run(2);
    assert!(
        entry.join("meta.toml").is_file(),
        "the second execution must write the directives back"
    );
    assert!(d2.out_dir.unwrap().join("generated.rs").is_file());

    // With the entry whole again the third run is a plain restore.
    let before = fs::metadata(entry.join("meta.toml"))
        .unwrap()
        .modified()
        .unwrap();
    let d3 = run(3);
    assert_eq!(
        fs::metadata(entry.join("meta.toml"))
            .unwrap()
            .modified()
            .unwrap(),
        before,
        "a restore must not rewrite the entry"
    );
    assert_eq!(
        fs::read_to_string(d3.out_dir.unwrap().join("generated.rs")).unwrap(),
        "pub const N: u32 = 7;"
    );
}
