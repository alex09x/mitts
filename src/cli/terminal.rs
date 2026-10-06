/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

#[cfg(unix)]
use std::cell::UnsafeCell;
#[cfg(unix)]
use std::mem::MaybeUninit;
#[cfg(unix)]
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

#[cfg(unix)]
struct TermiosStorage(UnsafeCell<MaybeUninit<libc::termios>>);
#[cfg(unix)]
unsafe impl Sync for TermiosStorage {}

#[cfg(unix)]
static HAS_ORIGINAL: AtomicBool = AtomicBool::new(false);
#[cfg(unix)]
static ORIGINAL_TERMIOS: TermiosStorage = TermiosStorage(UnsafeCell::new(MaybeUninit::uninit()));
#[cfg(unix)]
static PREV_SIGINT: AtomicUsize = AtomicUsize::new(0);
#[cfg(unix)]
static PREV_SIGTERM: AtomicUsize = AtomicUsize::new(0);

#[cfg(unix)]
extern "C" fn handle_signal(sig: libc::c_int) {
    restore_terminal();
    unsafe {
        libc::signal(sig, libc::SIG_DFL);
        libc::raise(sig);
    }
}

#[cfg(unix)]
fn restore_terminal() {
    if HAS_ORIGINAL.swap(false, Ordering::SeqCst) {
        unsafe {
            let fd = libc::STDIN_FILENO;
            // Discard any keystrokes (such as pressed Enter keys) accumulated during the build
            libc::tcflush(fd, libc::TCIFLUSH);
            // Restore original terminal attributes
            let orig_ptr = (*ORIGINAL_TERMIOS.0.get()).as_ptr();
            libc::tcsetattr(fd, libc::TCSANOW, orig_ptr);
            // Restore cursor visibility
            let _ = libc::write(libc::STDERR_FILENO, b"\x1b[?25h".as_ptr() as *const _, 6);

            // Restore previous signal handlers
            let prev_int = PREV_SIGINT.swap(0, Ordering::SeqCst);
            if prev_int != 0 {
                libc::signal(libc::SIGINT, prev_int as _);
            }
            let prev_term = PREV_SIGTERM.swap(0, Ordering::SeqCst);
            if prev_term != 0 {
                libc::signal(libc::SIGTERM, prev_term as _);
            }
        }
    }
}

/// RAII guard that disables terminal echoing and line buffering effects (preventing duplicate
/// progress bars when pressing Enter) and hides cursor during long-running compilation.
///
/// Restores original terminal modes, flushes unread input, and restores the cursor on drop
/// or on premature termination (SIGINT / SIGTERM).
pub struct TerminalGuard {
    active: bool,
}

impl TerminalGuard {
    /// Attempts to acquire control over the interactive terminal.
    /// If stdin is not a TTY, this is a safe no-op.
    pub fn acquire() -> Self {
        #[cfg(unix)]
        {
            let fd = libc::STDIN_FILENO;
            unsafe {
                if libc::isatty(fd) != 1 {
                    return Self { active: false };
                }
                if HAS_ORIGINAL
                    .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
                {
                    let mut termios = MaybeUninit::uninit();
                    if libc::tcgetattr(fd, termios.as_mut_ptr()) == 0 {
                        *ORIGINAL_TERMIOS.0.get() = termios;

                        // Save and set signal handlers
                        let sig_handler: extern "C" fn(libc::c_int) = handle_signal;
                        let prev_int = libc::signal(libc::SIGINT, sig_handler as _);
                        if prev_int != libc::SIG_ERR {
                            PREV_SIGINT.store(prev_int as usize, Ordering::SeqCst);
                        }
                        let prev_term = libc::signal(libc::SIGTERM, sig_handler as _);
                        if prev_term != libc::SIG_ERR {
                            PREV_SIGTERM.store(prev_term as usize, Ordering::SeqCst);
                        }

                        // Disable ECHO and ECHONL so pressing Enter or typing doesn't emit newlines
                        // or characters that clobber or duplicate the progress bar
                        let mut modified = (*ORIGINAL_TERMIOS.0.get()).assume_init();
                        modified.c_lflag &= !(libc::ECHO | libc::ECHONL);
                        libc::tcsetattr(fd, libc::TCSANOW, &modified);

                        // Hide cursor during progress bar rendering
                        let _ = libc::write(libc::STDERR_FILENO, b"\x1b[?25l".as_ptr() as *const _, 6);

                        return Self { active: true };
                    } else {
                        HAS_ORIGINAL.store(false, Ordering::SeqCst);
                    }
                }
            }
        }

        Self { active: false }
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if self.active {
            #[cfg(unix)]
            restore_terminal();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_terminal_guard_acquire_and_drop() {
        let guard = TerminalGuard::acquire();
        drop(guard);
    }
}
