/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! An executable a build script writes into `OUT_DIR` (a vendored `protoc`, as in tonic's
//! grpc-xds) must still run from the read-only store copy and after a restore.

#![cfg(unix)]

use mitts::adapter::rust::build_script::execute_build_script;
use mitts::adapter::rust::types::PlannedCompilationUnit;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

fn write_package(pkg_dir: &Path) {
    fs::create_dir_all(pkg_dir.join("src")).unwrap();
    fs::write(
        pkg_dir.join("Cargo.toml"),
        "[package]\nname = \"tool_pkg\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(pkg_dir.join("src/lib.rs"), "").unwrap();
    fs::write(
        pkg_dir.join("build.rs"),
        r##"use std::os::unix::fs::PermissionsExt;
fn main() {
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let tool = out.join("tool.sh");
    std::fs::write(&tool, "#!/bin/sh\necho tool-ok\n").unwrap();
    std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(out.join("data.txt"), "plain").unwrap();
}
"##,
    )
    .unwrap();
}

#[test]
fn executables_written_to_out_dir_keep_their_exec_bit_in_the_store() {
    let temp = TempDir::new().unwrap();
    let pkg_dir = temp.path().join("tool_pkg");
    write_package(&pkg_dir);
    let artifacts_dir = temp.path().join("cas/artifacts");
    fs::create_dir_all(&artifacts_dir).unwrap();
    let unit = PlannedCompilationUnit {
        package_id: "tool_pkg@0.1.0".to_string(),
        package_name: "tool_pkg".to_string(),
        version: "0.1.0".to_string(),
        target_name: "tool_pkg".to_string(),
        kind: "lib".to_string(),
        src_path: pkg_dir.join("src/lib.rs"),
        manifest_path: pkg_dir.join("Cargo.toml"),
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

    for n in 1..=2 {
        let out_dir = run(n).out_dir.unwrap();
        let tool = out_dir.join("tool.sh");
        let mode = fs::metadata(&tool).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o555,
            "run {n}: executable stays executable and read-only"
        );
        assert_eq!(
            fs::metadata(out_dir.join("data.txt"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o444,
            "run {n}: plain files stay read-only"
        );
        let out = Command::new(&tool).output().unwrap();
        assert!(out.status.success(), "run {n}: the stored tool must run");
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "tool-ok");
    }
}
