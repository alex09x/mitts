/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::core::JobserverPool;

#[test]
fn test_m8_jobserver_pool_managed_fifo_lifecycle() {
    let temp = tempfile::tempdir().unwrap();
    let cache_root = temp.path();

    // 1. Leader acquires or creates the pool with 4 tokens
    let leader_pool = JobserverPool::acquire_or_create(cache_root, 4).unwrap();
    let auth = leader_pool.auth().expect("Leader should have active auth");
    assert!(auth.starts_with("fifo:"));

    let fifo_path = cache_root.join("jobserver").join("pool.fifo");
    let lock_path = cache_root.join("jobserver").join("pool.lock");
    assert!(fifo_path.exists(), "FIFO should be created");
    assert!(lock_path.exists(), "Lock file should be created");

    // 2. Follower acquires the same pool while leader is active
    let follower_pool = JobserverPool::acquire_or_create(cache_root, 4).unwrap();
    let follower_auth = follower_pool.auth().expect("Follower should have active auth");
    assert_eq!(auth, follower_auth);

    // 3. Drop pools and verify leader re-acquisition can re-seed cleanly
    drop(follower_pool);
    drop(leader_pool);

    let reseeded_pool = JobserverPool::acquire_or_create(cache_root, 8).unwrap();
    assert!(reseeded_pool.auth().is_some());
}

#[test]
fn test_m8_jobserver_pool_inherited() {
    let temp = tempfile::tempdir().unwrap();
    let cache_root = temp.path();

    // Test inheriting from CARGO_MAKEFLAGS
    std::env::set_var(
        "CARGO_MAKEFLAGS",
        "--jobserver-auth=fifo:/tmp/mock_cargo.fifo --other-flag",
    );
    let pool = JobserverPool::acquire_or_create(cache_root, 4).unwrap();
    assert_eq!(pool.auth(), Some("fifo:/tmp/mock_cargo.fifo"));
    std::env::remove_var("CARGO_MAKEFLAGS");
}
