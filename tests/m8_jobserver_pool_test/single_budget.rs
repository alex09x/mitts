/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! One `-j` budget, as under Cargo: running units and the jobs they start (rustc, make,
//! cc-rs, all reading the pool FIFO) together stay within `-j`. Before, units were bounded
//! by the slot locks and nested jobs by a FIFO of `-j` tokens: about twice `-j` in all.

use mitts::core::JobserverPool;
use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

/// Tokens readable from the pool FIFO now; they are put back.
fn free_tokens(cache_root: &Path) -> usize {
    let path = cache_root.join("jobserver").join("pool.fifo");
    let c_path = std::ffi::CString::new(path.to_str().unwrap()).unwrap();
    let fd = unsafe {
        libc::open(
            c_path.as_ptr(),
            libc::O_RDWR | libc::O_NONBLOCK | libc::O_CLOEXEC,
        )
    };
    assert!(fd >= 0, "open {path:?}");
    let mut taken = Vec::new();
    let mut buf = [0u8; 64];
    loop {
        let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
        if n <= 0 {
            break;
        }
        taken.extend_from_slice(&buf[..n as usize]);
    }
    if !taken.is_empty() {
        let n = unsafe { libc::write(fd, taken.as_ptr().cast(), taken.len()) };
        assert_eq!(n as usize, taken.len());
    }
    unsafe { libc::close(fd) };
    taken.len()
}

#[test]
fn units_and_the_jobs_they_start_share_one_budget() {
    let temp = tempfile::tempdir().unwrap();
    let cache = temp.path().join("cache");
    let pool = JobserverPool::create_managed(&cache, 3).unwrap();
    // Like Cargo's pipe: `-j - 1` tokens, the first unit runs on the implicit token.
    assert_eq!(free_tokens(&cache), 2);

    let first = pool.acquire_token().unwrap().expect("a unit token");
    assert_eq!(
        free_tokens(&cache),
        2,
        "the first unit takes the implicit token"
    );
    let second = pool.acquire_token().unwrap().expect("a unit token");
    assert_eq!(
        free_tokens(&cache),
        1,
        "every further unit takes a FIFO token, so nested jobs get only what is left"
    );
    let third = pool.acquire_token().unwrap().expect("a unit token");
    assert_eq!(
        free_tokens(&cache),
        0,
        "three units: nothing left for nested jobs"
    );

    drop(second);
    assert_eq!(free_tokens(&cache), 1);
    drop(first);
    drop(third);
    assert_eq!(
        free_tokens(&cache),
        2,
        "every share came back, nothing more"
    );
}

/// A fourth unit waits until one of three ends (-j 3), and then runs.
#[test]
fn a_unit_past_the_budget_waits_for_one_to_end() {
    let temp = tempfile::tempdir().unwrap();
    let cache = temp.path().join("cache");
    // 'static, so a unit that never gets a token fails the test instead of hanging it.
    let pool: &'static JobserverPool =
        Box::leak(Box::new(JobserverPool::create_managed(&cache, 3).unwrap()));
    let held: Vec<_> = (0..3).map(|_| pool.acquire_token().unwrap()).collect();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let t = pool.acquire_token().unwrap();
        tx.send(()).unwrap();
        drop(t);
    });
    assert!(
        rx.recv_timeout(Duration::from_millis(300)).is_err(),
        "a fourth unit ran within -j 3"
    );
    drop(held);
    rx.recv_timeout(Duration::from_secs(10))
        .expect("the waiting unit runs once one ends");
}

const CHILD_ENV: &str = "MITTS_TEST_JOBSERVER_CHILD_CACHE";

/// Child half of the test below: joins the pool, takes the implicit token and a FIFO
/// token, and exits without giving either back (no destructors run), like a killed build.
#[test]
fn child_takes_two_unit_tokens_and_dies() {
    let Some(cache) = std::env::var_os(CHILD_ENV) else {
        return;
    };
    let pool = JobserverPool::create_managed(Path::new(&cache), 3).unwrap();
    let a = pool.acquire_token().unwrap();
    let b = pool.acquire_token().unwrap();
    std::mem::forget((a, b));
    std::process::exit(0);
}

/// The pool FIFO outlives a Mitts process: a token held by a process that died must come
/// back, or every later build has less parallelism, down to none. The slot lock the kernel
/// releases carries a record of the token, and the slot's next holder puts it back.
#[test]
fn a_token_held_by_a_dead_process_comes_back() {
    let temp = tempfile::tempdir().unwrap();
    let cache = temp.path().join("cache");
    let pool: &'static JobserverPool =
        Box::leak(Box::new(JobserverPool::create_managed(&cache, 3).unwrap()));

    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "single_budget::child_takes_two_unit_tokens_and_dies",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD_ENV, &cache)
        .status()
        .unwrap();
    assert!(status.success());
    assert_eq!(free_tokens(&cache), 1, "the dead child kept one FIFO token");

    // Three units (-j 3) still run: one on the implicit token, two on FIFO tokens, one of
    // them the token the child left behind.
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let held: Vec<_> = (0..3).map(|_| pool.acquire_token().unwrap()).collect();
        tx.send(()).unwrap();
        drop(held);
        tx.send(()).unwrap();
    });
    rx.recv_timeout(Duration::from_secs(10))
        .expect("the lost token came back: three units ran");
    rx.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(free_tokens(&cache), 2, "back to -j - 1, not more");
}
