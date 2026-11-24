/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! The Windows jobserver, as GNU Make and Cargo have it there: a named semaphore whose
//! count is the free tokens. `--jobserver-auth=NAME` names it; a client (rustc, the `cc` and
//! `jobserver` crates) opens it by name, waits on it for a token and releases it to give the
//! token back.

use std::ffi::c_void;

type Handle = *mut c_void;

#[link(name = "kernel32")]
extern "system" {
    fn CreateSemaphoreW(
        attributes: *mut c_void,
        initial: i32,
        maximum: i32,
        name: *const u16,
    ) -> Handle;
    fn OpenSemaphoreW(access: u32, inherit: i32, name: *const u16) -> Handle;
    fn WaitForSingleObject(handle: Handle, milliseconds: u32) -> u32;
    fn ReleaseSemaphore(handle: Handle, count: i32, previous: *mut i32) -> i32;
    fn CloseHandle(handle: Handle) -> i32;
}

const SYNCHRONIZE: u32 = 0x0010_0000;
const SEMAPHORE_MODIFY_STATE: u32 = 0x0002;
const INFINITE: u32 = 0xFFFF_FFFF;
const WAIT_OBJECT_0: u32 = 0;

fn wide(name: &str) -> Vec<u16> {
    name.encode_utf16().chain(std::iter::once(0)).collect()
}

/// An open semaphore handle, closed on drop.
#[derive(Debug)]
pub struct Semaphore(Handle);

// A kernel handle may be used from any thread.
unsafe impl Send for Semaphore {}
unsafe impl Sync for Semaphore {}

impl Semaphore {
    /// A new semaphore named `name` holding `tokens` tokens.
    pub fn create(name: &str, tokens: usize) -> std::io::Result<Semaphore> {
        let count = i32::try_from(tokens.max(1)).unwrap_or(i32::MAX);
        let name = wide(name);
        let handle = unsafe { CreateSemaphoreW(std::ptr::null_mut(), count, count, name.as_ptr()) };
        if handle.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        Ok(Semaphore(handle))
    }

    /// The semaphore another process created under `name`.
    pub fn open(name: &str) -> std::io::Result<Semaphore> {
        let name = wide(name);
        let handle =
            unsafe { OpenSemaphoreW(SYNCHRONIZE | SEMAPHORE_MODIFY_STATE, 0, name.as_ptr()) };
        if handle.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        Ok(Semaphore(handle))
    }

    /// Waits for a token.
    pub fn acquire(&self) -> std::io::Result<()> {
        match unsafe { WaitForSingleObject(self.0, INFINITE) } {
            WAIT_OBJECT_0 => Ok(()),
            _ => Err(std::io::Error::last_os_error()),
        }
    }

    /// Gives a token back.
    pub fn release(&self) -> std::io::Result<()> {
        if unsafe { ReleaseSemaphore(self.0, 1, std::ptr::null_mut()) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
}

impl Drop for Semaphore {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

/// A fresh semaphore name for this process, in the style Cargo's `jobserver` crate uses.
pub fn fresh_name() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    format!("__mitts_jobserver_semaphore_{}_{nanos}", std::process::id())
}
