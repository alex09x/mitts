/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::dag::CompilationDag;
use super::dag_cache::ExecutionContext;
use super::dag_scheduler::{UnitExecutionStatus, WaveWorkQueue};
use super::dag_unit::execute_unit;
use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::AtomicUsize;
use std::sync::Mutex;

struct WorkGuard<'a> {
    queue: &'a WaveWorkQueue,
    unit_id: String,
    resolved: bool,
}

impl<'a> Drop for WorkGuard<'a> {
    fn drop(&mut self) {
        if !self.resolved {
            self.queue.mark_failed(format!(
                "Worker thread aborted or panicked while executing unit '{}'",
                self.unit_id
            ));
        }
    }
}

/// Shared compilation DAG state accessed by parallel worker threads.
pub(crate) struct WorkerShared<'a> {
    pub dag: &'a CompilationDag,
    pub work_queue: &'a WaveWorkQueue,
    pub in_degrees: &'a HashMap<String, AtomicUsize>,
    pub metadata_signaled: &'a Mutex<HashSet<String>>,
    pub resolved_keys: &'a Mutex<HashMap<String, String>>,
    pub unit_keys: &'a HashMap<String, String>,
    pub deriver: &'a super::key::UnitKeyDeriver,
}

/// Worker thread execution loop consuming compilation units from the work queue.
pub(crate) fn run_worker_loop(shared: &WorkerShared<'_>, ctx: &ExecutionContext<'_>) -> Result<()> {
    while let Some((unit_id, lock_mode)) = shared.work_queue.pop_work() {
        let mut guard = WorkGuard {
            queue: shared.work_queue,
            unit_id: unit_id.clone(),
            resolved: false,
        };

        let Some(node) = shared.dag.get_node(&unit_id) else {
            guard.resolved = true;
            shared.work_queue.mark_completed(&unit_id);
            continue;
        };
        let unit = node.unit.clone();
        let dep_keys: Vec<(String, String)> = {
            let map = shared.resolved_keys.lock().unwrap();
            unit.dependencies
                .iter()
                .filter_map(|dep_id| {
                    map.get(dep_id)
                        .or_else(|| shared.unit_keys.get(dep_id))
                        .map(|k| (dep_id.clone(), k.clone()))
                })
                .collect()
        };
        let dep_refs: Vec<(&str, &str)> = dep_keys
            .iter()
            .map(|(id, k)| (id.as_str(), k.as_str()))
            .collect();
        let action_key = shared.deriver.compute_dynamic_key(&unit, &dep_refs);

        let exec_res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            execute_unit(&unit, &action_key, ctx, lock_mode)
        }));

        match exec_res {
            Ok(Ok(UnitExecutionStatus::Success)) => {
                guard.resolved = true;
                let was_meta_signaled = shared.metadata_signaled.lock().unwrap().contains(&unit_id);
                if let Some(node) = shared.dag.get_node(&unit_id) {
                    for dep_id in &node.dependents {
                        if let Some(dep_node) = shared.dag.get_node(dep_id) {
                            let req = super::pipelining::classify_dep_requirement(
                                &dep_node.unit,
                                &node.unit,
                            );
                            if was_meta_signaled
                                && req == super::pipelining::DepRequirement::MetadataOnly
                            {
                                continue;
                            }
                            if let Some(counter) = shared.in_degrees.get(dep_id) {
                                if counter.fetch_sub(1, std::sync::atomic::Ordering::SeqCst) == 1 {
                                    shared.work_queue.push_ready(dep_id.clone());
                                }
                            }
                        }
                    }
                }
                shared.work_queue.mark_completed(&unit_id);
            }
            Ok(Ok(UnitExecutionStatus::Contested)) => {
                guard.resolved = true;
                shared.work_queue.mark_contested(unit_id);
            }
            Ok(Err(err)) => {
                guard.resolved = true;
                shared.work_queue.mark_failed(format!("{:#}", err));
                break;
            }
            Err(panic_err) => {
                guard.resolved = true;
                let msg = if let Some(s) = panic_err.downcast_ref::<&str>() {
                    s.to_string()
                } else if let Some(s) = panic_err.downcast_ref::<String>() {
                    s.clone()
                } else {
                    "Unknown worker thread panic".to_string()
                };
                shared.work_queue.mark_failed(format!(
                    "Worker thread panicked on unit {}: {}",
                    unit_id, msg
                ));
                break;
            }
        }
    }
    Ok(())
}
