/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::rust::types::RustBuildOptions;
use mitts::adapter::rust::RustAdapter;
use mitts::core::JobserverPool;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::mpsc;
use std::time::Duration;

#[test]
#[cfg(unix)]
fn test_h2_managed_capacity_1_enforces_global_cap_across_distinct_projects() {
    if let (Ok(proj_dir), Ok(cache_dir)) = (
        std::env::var("MITTS_TEST_H2_CHILD_PROJECT"),
        std::env::var("MITTS_TEST_H2_CHILD_CACHE"),
    ) {
        let dir = PathBuf::from(proj_dir);
        let cache = PathBuf::from(cache_dir);
        JobserverPool::reset_tokens_acquired();
        let opt = RustBuildOptions {
            manifest_path: Some(dir.join("Cargo.toml")),
            target_dir: Some(dir.join("target")),
            cache_dir: Some(cache),
            native: true,
            ..Default::default()
        };
        let (_, res) = RustAdapter::build_native(&opt).unwrap();
        assert!(res.success, "child build failed for {}", dir.display());
        assert!(
            JobserverPool::total_tokens_acquired() >= 1,
            "child managed build must acquire at least 1 managed slot token"
        );
        return;
    }

    let temp = tempfile::tempdir().unwrap();
    let cache_root = temp.path().join("cache");
    fs::create_dir_all(&cache_root).unwrap();

    // Leader creates a shared managed pool with capacity = 1 and holds the single slot
    let leader = JobserverPool::create_managed(&cache_root, 1).unwrap();
    assert!(!leader.has_implicit_token());
    let held_slot = leader.acquire_token().unwrap();
    assert!(held_slot.is_some());

    // Create two distinct projects with different names/sources so single-flight locks do not overlap
    for (name, val) in [("proj_alpha", 11u32), ("proj_beta", 22u32)] {
        let dir = temp.path().join(name);
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(
            dir.join("Cargo.toml"),
            format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
        )
        .unwrap();
        fs::write(
            dir.join("src/lib.rs"),
            format!("pub fn val() -> u32 {{ {val} }}\n"),
        )
        .unwrap();
        fs::write(
            dir.join("Cargo.lock"),
            format!("version = 3\n[[package]]\nname = \"{name}\"\nversion = \"0.1.0\"\n"),
        )
        .unwrap();
    }

    let spawn_isolated_child = |proj_name: &str| {
        let proj_dir = temp.path().join(proj_name);
        let cache = cache_root.clone();
        let (tx, rx) = mpsc::channel();
        let handle = std::thread::spawn(move || {
            let status = Command::new(std::env::current_exe().unwrap())
                .arg("--exact")
                .arg("managed_cap::test_h2_managed_capacity_1_enforces_global_cap_across_distinct_projects")
                .arg("--nocapture")
                .env_remove("CARGO_MAKEFLAGS")
                .env_remove("MAKEFLAGS")
                .env("MITTS_TEST_H2_CHILD_PROJECT", &proj_dir)
                .env("MITTS_TEST_H2_CHILD_CACHE", &cache)
                .status()
                .unwrap();
            tx.send(status.success()).unwrap();
        });
        (rx, handle)
    };

    let (rx_a, handle_a) = spawn_isolated_child("proj_alpha");
    let (rx_b, handle_b) = spawn_isolated_child("proj_beta");

    // Neither child process may bypass the held slot_0.lock via an unslotted per-process implicit job
    assert!(
        rx_a.recv_timeout(Duration::from_millis(350)).is_err(),
        "proj_alpha bypassed managed capacity=1 while slot_0 was held"
    );
    assert!(
        rx_b.recv_timeout(Duration::from_millis(50)).is_err(),
        "proj_beta bypassed managed capacity=1 while slot_0 was held"
    );

    // Release slot_0: both distinct child builds must now acquire managed slots sequentially and succeed
    drop(held_slot);
    assert!(rx_a.recv_timeout(Duration::from_secs(15)).unwrap());
    assert!(rx_b.recv_timeout(Duration::from_secs(15)).unwrap());
    handle_a.join().unwrap();
    handle_b.join().unwrap();
    drop(leader);
}
