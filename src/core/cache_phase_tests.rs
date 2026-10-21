/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::*;
use std::sync::mpsc;
use std::time::Duration;

#[test]
fn paused_compiler_does_not_block_cache_and_resume_waits() {
    let budget = CachePhaseBudget::new(1);
    let _scope = budget.activate();
    let _unit = enter_unit();
    let (started_tx, started_rx) = mpsc::channel();
    let (acquired_tx, acquired_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let worker_budget = budget.clone();
    let worker = std::thread::spawn(move || {
        let _scope = worker_budget.activate();
        started_tx.send(()).unwrap();
        let _unit = enter_unit();
        acquired_tx.send(()).unwrap();
        release_rx.recv().unwrap();
    });
    started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(acquired_rx.recv_timeout(Duration::from_millis(30)).is_err());
    pause();
    acquired_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    // Second unit holds the only ticket; resume must wait until it releases it.
    let release = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(30));
        release_tx.send(()).unwrap();
    });
    resume();
    worker.join().unwrap();
    release.join().unwrap();
    assert_eq!(*budget.0.as_ref().unwrap().available.lock().unwrap(), 0);
}
#[test]
fn panic_releases_ticket_and_restores_thread_state() {
    let budget = CachePhaseBudget::new(1);
    let result = std::panic::catch_unwind(|| {
        let _scope = budget.activate();
        let _unit = enter_unit();
        panic!("deliberate unit failure");
    });
    assert!(result.is_err());
    assert_eq!(*budget.0.as_ref().unwrap().available.lock().unwrap(), 1);
    let _scope = budget.activate();
    let _unit = enter_unit();
    pause();
    pause();
    resume();
    resume();
    assert_eq!(*budget.0.as_ref().unwrap().available.lock().unwrap(), 0);
}
#[test]
fn disabled_budget_and_nested_inactive_activation_are_noops() {
    let budget = CachePhaseBudget::new(0);
    let _outer = budget.activate();
    {
        let _inner = CachePhaseBudget::new(1).activate();
        let _unit = enter_unit();
    }
    let _unit = enter_unit();
    pause();
    resume();
    STATE.with(|s| assert!(!s.borrow().held));
}
