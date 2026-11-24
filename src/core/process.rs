/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Whether a process that left something behind (a staging directory, a lock) is gone.
//! Only a definite "no such process" counts: anything uncertain (no permission to look)
//! keeps the process alive, so its files are never removed under it.

/// Whether no process with `pid` exists any more.
#[cfg(unix)]
pub fn is_dead(pid: u32) -> bool {
    // Signal 0 only checks for existence.
    unsafe {
        libc::kill(pid as libc::pid_t, 0) == -1
            && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
    }
}

/// Whether no process with `pid` exists any more: it cannot be opened (Windows reports an
/// invalid parameter for an unknown id), or it is open but has exited.
#[cfg(windows)]
pub fn is_dead(pid: u32) -> bool {
    use std::ffi::c_void;
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const ERROR_INVALID_PARAMETER: i32 = 87;
    const STILL_ACTIVE: u32 = 259;
    #[link(name = "kernel32")]
    extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
        fn GetExitCodeProcess(process: *mut c_void, code: *mut u32) -> i32;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return std::io::Error::last_os_error().raw_os_error() == Some(ERROR_INVALID_PARAMETER);
        }
        let mut code = 0u32;
        let exited = GetExitCodeProcess(handle, &mut code) != 0 && code != STILL_ACTIVE;
        CloseHandle(handle);
        exited
    }
}

/// Elsewhere there is no way to tell: never dead.
#[cfg(not(any(unix, windows)))]
pub fn is_dead(_pid: u32) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_process_is_alive_and_a_finished_child_is_dead() {
        assert!(!is_dead(std::process::id()));
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("--list")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id();
        child.wait().unwrap();
        // Reaped on unix; exited (and the id not reused this fast) on Windows.
        assert!(is_dead(pid));
    }
}
