/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::cell::RefCell;
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::{Arc, Condvar, Mutex};

/// Per-DAG budget for cache I/O; compilation and blocking locks do not hold it.
#[derive(Clone)]
pub(crate) struct CachePhaseBudget(Option<Arc<Gate>>);
struct Gate {
    available: Mutex<usize>,
    wake: Condvar,
}
impl Gate {
    fn acquire(&self) {
        let mut available = self.available.lock().unwrap();
        while *available == 0 {
            available = self.wake.wait(available).unwrap();
        }
        *available -= 1;
    }
    fn release(&self) {
        *self.available.lock().unwrap() += 1;
        self.wake.notify_one();
    }
}
#[derive(Default)]
struct State {
    gate: Option<Arc<Gate>>,
    active: bool,
    held: bool,
}
thread_local! { static STATE: RefCell<State> = RefCell::new(State::default()); }
impl CachePhaseBudget {
    pub(crate) fn new(limit: usize) -> Self {
        Self((limit > 0).then(|| {
            Arc::new(Gate {
                available: Mutex::new(limit),
                wake: Condvar::new(),
            })
        }))
    }
    pub(crate) fn for_platform() -> Self {
        Self::new(if cfg!(target_os = "macos") { 1 } else { 0 })
    }
    pub(crate) fn activate(&self) -> BudgetScope {
        let previous = STATE.with(|s| {
            let mut state = s.borrow_mut();
            assert!(!state.active, "cannot replace an active cache phase budget");
            std::mem::replace(
                &mut *state,
                State {
                    gate: self.0.clone(),
                    ..State::default()
                },
            )
        });
        BudgetScope {
            previous: Some(previous),
            _thread: PhantomData,
        }
    }
}
pub(crate) struct BudgetScope {
    previous: Option<State>,
    _thread: PhantomData<Rc<()>>,
}
impl Drop for BudgetScope {
    fn drop(&mut self) {
        pause();
        STATE.with(|s| *s.borrow_mut() = self.previous.take().unwrap());
    }
}
pub(crate) struct UnitScope(PhantomData<Rc<()>>);
pub(crate) fn enter_unit() -> UnitScope {
    STATE.with(|s| {
        let mut state = s.borrow_mut();
        assert!(!state.active, "nested unit cache phases are unsupported");
        state.active = true;
    });
    resume();
    UnitScope(PhantomData)
}
impl Drop for UnitScope {
    fn drop(&mut self) {
        pause();
        STATE.with(|s| s.borrow_mut().active = false);
    }
}
/// Release cache capacity before waiting on a compiler token or single-flight lock.
pub(crate) fn pause() {
    let gate = STATE.with(|s| {
        let mut state = s.borrow_mut();
        if !state.held {
            return None;
        }
        state.held = false;
        state.gate.clone()
    });
    if let Some(gate) = gate {
        gate.release();
    }
}
/// Re-enter after compilation or blocking waits; never blocks with a RefCell borrow.
pub(crate) fn resume() {
    let gate = STATE.with(|s| {
        let state = s.borrow();
        if state.active && !state.held {
            state.gate.clone()
        } else {
            None
        }
    });
    if let Some(gate) = gate {
        gate.acquire();
        STATE.with(|s| s.borrow_mut().held = true);
    }
}
#[cfg(test)]
#[path = "cache_phase_tests.rs"]
mod tests;
