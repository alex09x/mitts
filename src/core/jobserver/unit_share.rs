/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! One `-j` budget for units and the tools they start, as under Cargo.
//!
//! The managed pool holds `-j` slot locks and a FIFO preloaded with `-j - 1` tokens. A
//! running unit holds a slot and its share of the budget: the process's implicit token
//! (the first unit, like Cargo's own) or one FIFO token. rustc, make and cc-rs inside the
//! unit read further tokens from the same FIFO, so units plus nested jobs stay within `-j`
//! per Mitts process.
//!
//! The FIFO is shared by Mitts processes, and a process killed while holding a FIFO token
//! would lose it for every later build. The slot file records that its holder has a FIFO
//! token; the kernel releases the slot lock of a dead process, and whoever takes the slot
//! next finds the record and puts the token back.

use super::managed_slots::ManagedSlotLease;
use anyhow::Result;
use std::fs::File;
use std::os::raw::c_int;
use std::os::unix::fs::FileExt;
use std::sync::atomic::{AtomicBool, Ordering};

const HELD: u8 = b'1';
const FREE: u8 = b'0';
const TOKEN: u8 = b'|';

/// A running unit's share of the budget.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UnitShare {
    /// The process's implicit token: nothing taken from the FIFO.
    Implicit,
    /// One token read from the FIFO.
    Token(u8),
}

fn record(slot: &File) -> Option<u8> {
    let mut b = [0u8];
    matches!(slot.read_at(&mut b, 0), Ok(1)).then_some(b[0])
}

fn set_record(slot: &File, value: u8) {
    let _ = slot.write_at(&[value], 0);
}

fn write_token(fd: c_int, byte: u8) {
    loop {
        let n = unsafe { libc::write(fd, &byte as *const u8 as *const libc::c_void, 1) };
        if n == 1 {
            return;
        }
        let err = std::io::Error::last_os_error();
        if n < 0 && err.raw_os_error() == Some(libc::EINTR) {
            continue;
        }
        eprintln!("mitts: warning: failed to release jobserver token: {err}");
        return;
    }
}

/// One FIFO token if one is there now.
fn try_read_token(fd: c_int) -> Result<Option<u8>> {
    let mut byte = 0u8;
    let n = unsafe { libc::read(fd, &mut byte as *mut u8 as *mut libc::c_void, 1) };
    if n == 1 {
        return Ok(Some(byte));
    }
    if n == 0 {
        // No writer can be gone: this process keeps the FIFO open read-write.
        return Ok(None);
    }
    let err = std::io::Error::last_os_error();
    match err.raw_os_error() {
        Some(libc::EAGAIN) | Some(libc::EINTR) => Ok(None),
        _ => Err(anyhow::anyhow!("Failed to acquire jobserver token: {err}")),
    }
}

/// Waits up to `ms` for the FIFO to become readable.
fn wait_readable(fd: c_int, ms: c_int) {
    let mut p = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    unsafe { libc::poll(&mut p, 1, ms) };
}

/// Takes the unit's share while it holds `slot`. `fd` is this process's non-blocking
/// descriptor of the pool FIFO; `implicit` is the process's implicit token.
pub(crate) fn acquire(
    fd: c_int,
    implicit: &AtomicBool,
    slot: &ManagedSlotLease,
) -> Result<UnitShare> {
    let file = slot.file();
    // The slot's last holder died holding a FIFO token: put it back.
    if record(file) == Some(HELD) {
        write_token(fd, TOKEN);
        set_record(file, FREE);
    }
    loop {
        if implicit
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
        {
            return Ok(UnitShare::Implicit);
        }
        if let Some(byte) = try_read_token(fd)? {
            set_record(file, HELD);
            return Ok(UnitShare::Token(byte));
        }
        // Wake for a token, and now and then for the implicit token coming back.
        wait_readable(fd, 20);
    }
}

/// Gives the share back before the slot is unlocked.
pub(crate) fn release(fd: c_int, implicit: &AtomicBool, slot: &ManagedSlotLease, share: UnitShare) {
    match share {
        UnitShare::Implicit => implicit.store(false, Ordering::Release),
        UnitShare::Token(byte) => {
            write_token(fd, byte);
            set_record(slot.file(), FREE);
        }
    }
}

/// Makes `fd` non-blocking, so a reader that lost the race for a token does not hang.
pub(crate) fn set_nonblocking(fd: c_int) {
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        if flags >= 0 {
            libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
        }
    }
}
