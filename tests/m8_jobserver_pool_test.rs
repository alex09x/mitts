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
    let leader_pool = JobserverPool::create_managed(cache_root, 4).unwrap();
    let auth = leader_pool.auth().expect("Leader should have active auth");
    assert!(auth.starts_with("fifo:"));

    let fifo_path = cache_root.join("jobserver").join("pool.fifo");
    let lock_path = cache_root.join("jobserver").join("pool.lock");
    assert!(fifo_path.exists(), "FIFO should be created");
    assert!(lock_path.exists(), "Lock file should be created");

    // 2. Follower acquires the same pool while leader is active
    let follower_pool = JobserverPool::create_managed(cache_root, 4).unwrap();
    let follower_auth = follower_pool
        .auth()
        .expect("Follower should have active auth");
    assert_eq!(auth, follower_auth);

    // 3. Token acquire and release via RAII guard
    let token1 = leader_pool.acquire_token().unwrap();
    assert!(token1.is_some());
    drop(token1);

    // 4. Drop pools and verify leader re-acquisition can re-seed cleanly
    drop(follower_pool);
    drop(leader_pool);

    let reseeded_pool = JobserverPool::create_managed(cache_root, 8).unwrap();
    assert!(reseeded_pool.auth().is_some());
}

#[test]
fn test_m8_jobserver_pool_inherited_fifo() {
    let temp = tempfile::tempdir().unwrap();

    // Create a real FIFO on disk
    let fifo_path = temp.path().join("inherited.fifo");
    let c_path = std::ffi::CString::new(fifo_path.to_str().unwrap()).unwrap();
    unsafe {
        let res = libc::mkfifo(c_path.as_ptr(), 0o600);
        assert_eq!(res, 0);
    }

    // Open inherited pool (keeps FIFO open so kernel pipe buffer is preserved)
    let auth_str = format!("fifo:{}", fifo_path.display());
    let pool = JobserverPool::from_auth(&auth_str);
    assert_eq!(pool.auth(), Some(auth_str.as_str()));

    // Seed 1 token into the FIFO
    unsafe {
        let fd = libc::open(c_path.as_ptr(), libc::O_WRONLY | libc::O_CLOEXEC);
        assert!(fd >= 0);
        let buf = b"+";
        let n = libc::write(fd, buf.as_ptr() as *const libc::c_void, 1);
        assert_eq!(n, 1);
        libc::close(fd);
    }

    // Verify token can be acquired from the inherited FIFO
    let token = pool.acquire_token().unwrap();
    assert!(token.is_some());
    drop(token);

    // Verify token was released back and can be re-acquired
    let token2 = pool.acquire_token().unwrap();
    assert!(token2.is_some());
}

#[test]
fn test_m8_jobserver_pool_inherited_pipe_fds() {
    #[cfg(unix)]
    {
        let mut fds = [0 as std::os::raw::c_int; 2];
        let res = unsafe { libc::pipe(fds.as_mut_ptr()) };
        assert_eq!(res, 0);
        let (r, w) = (fds[0], fds[1]);

        let buf = b"+";
        unsafe {
            libc::write(w, buf.as_ptr() as *const libc::c_void, 1);
        }

        let auth_str = format!("{},{}", r, w);
        let pool = JobserverPool::from_auth(&auth_str);
        assert_eq!(pool.auth(), Some(auth_str.as_str()));

        let token = pool.acquire_token().unwrap();
        assert!(token.is_some());
        drop(token);

        unsafe {
            libc::close(r);
            libc::close(w);
        }
    }
}

#[test]
fn test_m8_jobserver_pool_none() {
    let none_pool = JobserverPool::None;
    assert!(none_pool.auth().is_none());
    assert!(none_pool.acquire_token().unwrap().is_none());
    none_pool.release_token(b'+');
}
