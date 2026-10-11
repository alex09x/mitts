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
use super::types::{BuildProgressEvent, EffectivePlanExplanation, RustBuildOptions};
use crate::core::action::{Action, ActionResult, BuildPlan};
use crate::core::action_cache::ActionCache;
use crate::core::cas::ContentAddressedStore;
use anyhow::Result;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;

/// Executes the compilation DAG natively using direct `rustc` invocations in parallel.
/// Intermediates live exclusively in the Global CAS (zero workspace target pollution).
pub fn run_native_dag_build(
    opt: &RustBuildOptions,
    explanation: &EffectivePlanExplanation,
    deriver: &super::key::UnitKeyDeriver,
) -> Result<(BuildPlan, ActionResult)> {
    let unit_keys = &deriver.initial_base_keys;
    let cas = match opt.cache_dir {
        Some(ref cd) => ContentAddressedStore::new(cd.join("cas"))?,
        None => ContentAddressedStore::default_store()?,
    };
    let ac = match opt.cache_dir {
        Some(ref cd) => ActionCache::new(cd.join("ac"))?,
        None => ActionCache::default_cache()?,
    };

    let dag = CompilationDag::from_explanation(explanation)?;
    let _ = dag.topological_waves()?;

    let completed_artifacts: Arc<Mutex<HashMap<String, PathBuf>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let completed_cfgs: Arc<Mutex<HashMap<String, Vec<String>>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let recorded_actions: Arc<Mutex<Vec<Action>>> = Arc::new(Mutex::new(Vec::new()));
    let compiler_messages: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let any_missed: Arc<Mutex<bool>> = Arc::new(Mutex::new(false));
    let dep_build_metadata: Arc<Mutex<HashMap<String, HashMap<String, String>>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let resolved_keys: Arc<Mutex<HashMap<String, String>>> = Arc::new(Mutex::new(HashMap::new()));

    let target_dir = explanation.target_directory.clone();
    fs::create_dir_all(&target_dir)?;

    let is_cache_enabled = !opt.no_cache;

    if let Some(ref cb) = opt.progress {
        cb.0(BuildProgressEvent::InitTotal {
            total: explanation.units.len(),
            cached: 0,
        });
    }

    let max_concurrency = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(8);

    let total_units = dag.len();
    let initial_leaves = dag.leaf_ids().to_vec();
    let work_queue = Arc::new(WaveWorkQueue::with_total(initial_leaves, total_units));

    let in_degrees: Arc<HashMap<String, std::sync::atomic::AtomicUsize>> = Arc::new(
        dag.node_ids()
            .into_iter()
            .map(|id| {
                let deg = dag.get_node(&id).map(|n| n.dependencies.len()).unwrap_or(0);
                (id, std::sync::atomic::AtomicUsize::new(deg))
            })
            .collect(),
    );

    let mut compile_errors: Vec<String> = Vec::new();
    let concurrency = max_concurrency.min(total_units.max(1));

    let cache_root = cas.root().parent().unwrap_or_else(|| cas.root());
    let jobserver = crate::core::JobserverPool::acquire_or_create(cache_root, max_concurrency)
        .ok()
        .map(Arc::new);
    let is_first_job = Arc::new(std::sync::atomic::AtomicBool::new(true));

    let metadata_signaled = Arc::new(std::sync::Mutex::new(std::collections::HashSet::new()));

    let on_metadata_ready = {
        let dag = &dag;
        let in_degrees = Arc::clone(&in_degrees);
        let work_queue = Arc::clone(&work_queue);
        let metadata_signaled = Arc::clone(&metadata_signaled);
        move |completed_unit_id: &str, _rmeta_path: &std::path::Path| {
            let mut sig = metadata_signaled.lock().unwrap();
            if !sig.insert(completed_unit_id.to_string()) {
                return;
            }
            drop(sig);

            if let Some(node) = dag.get_node(completed_unit_id) {
                for dep_id in &node.dependents {
                    if let Some(dep_node) = dag.get_node(dep_id) {
                        if super::pipelining::classify_dep_requirement(&dep_node.unit, &node.unit)
                            == super::pipelining::DepRequirement::MetadataOnly
                        {
                            if let Some(counter) = in_degrees.get(dep_id) {
                                if counter.fetch_sub(1, std::sync::atomic::Ordering::SeqCst) == 1 {
                                    work_queue.push_ready(dep_id.clone());
                                }
                            }
                        }
                    }
                }
            }
        }
    };

    let compiled_actions = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let cached_actions = Arc::new(std::sync::atomic::AtomicUsize::new(0));

    thread::scope(|s| {
        let mut handles = Vec::new();

        for _ in 0..concurrency {
            let work_queue = Arc::clone(&work_queue);
            let in_degrees = Arc::clone(&in_degrees);
            let metadata_signaled = Arc::clone(&metadata_signaled);
            let on_metadata_ready = &on_metadata_ready;
            let cas = cas.clone();
            let ac = ac.clone();
            let completed_artifacts = Arc::clone(&completed_artifacts);
            let completed_cfgs = Arc::clone(&completed_cfgs);
            let recorded_actions = Arc::clone(&recorded_actions);
            let compiler_messages = Arc::clone(&compiler_messages);
            let any_missed = Arc::clone(&any_missed);
            let dep_build_metadata = Arc::clone(&dep_build_metadata);
            let resolved_keys = Arc::clone(&resolved_keys);
            let deriver = deriver.clone();
            let jobserver = jobserver.clone();
            let is_first_job = Arc::clone(&is_first_job);
            let target_dir = target_dir.clone();
            let toolchain = explanation.toolchain.clone();
            let opt = opt.clone();
            let dag = &dag;
            let compiled_actions = Arc::clone(&compiled_actions);
            let cached_actions = Arc::clone(&cached_actions);

            handles.push(s.spawn(move || -> Result<()> {
                let ctx = ExecutionContext {
                    opt: &opt,
                    toolchain: &toolchain,
                    target_dir: &target_dir,
                    cas: &cas,
                    ac: &ac,
                    is_cache_enabled,
                    completed_artifacts: &completed_artifacts,
                    recorded_actions: &recorded_actions,
                    compiler_messages: &compiler_messages,
                    any_missed: &any_missed,
                    dep_build_metadata: &dep_build_metadata,
                    resolved_keys: Some(&resolved_keys),
                    jobserver: jobserver.as_ref(),
                    is_first_job: Some(&is_first_job),
                    on_metadata_ready: Some(on_metadata_ready),
                    compiled_actions: Some(&compiled_actions),
                    cached_actions: Some(&cached_actions),
                    completed_cfgs: Some(&completed_cfgs),
                };

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

                while let Some((unit_id, lock_mode)) = work_queue.pop_work() {
                    let mut guard = WorkGuard {
                        queue: &work_queue,
                        unit_id: unit_id.clone(),
                        resolved: false,
                    };

                    let Some(node) = dag.get_node(&unit_id) else {
                        guard.resolved = true;
                        work_queue.mark_completed(&unit_id);
                        continue;
                    };
                    let unit = node.unit.clone();
                    let dep_keys: Vec<(String, String)> = {
                        let map = resolved_keys.lock().unwrap();
                        unit.dependencies
                            .iter()
                            .filter_map(|dep_id| {
                                map.get(dep_id)
                                    .or_else(|| unit_keys.get(dep_id))
                                    .map(|k| (dep_id.clone(), k.clone()))
                            })
                            .collect()
                    };
                    let dep_refs: Vec<(&str, &str)> = dep_keys
                        .iter()
                        .map(|(id, k)| (id.as_str(), k.as_str()))
                        .collect();
                    let action_key = deriver.compute_dynamic_key(&unit, &dep_refs);

                    let exec_res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        execute_unit(&unit, &action_key, &ctx, lock_mode)
                    }));

                    match exec_res {
                        Ok(Ok(UnitExecutionStatus::Success)) => {
                            guard.resolved = true;
                            let was_meta_signaled =
                                metadata_signaled.lock().unwrap().contains(&unit_id);
                            if let Some(node) = dag.get_node(&unit_id) {
                                for dep_id in &node.dependents {
                                    if let Some(dep_node) = dag.get_node(dep_id) {
                                        let req = super::pipelining::classify_dep_requirement(
                                            &dep_node.unit,
                                            &node.unit,
                                        );
                                        if was_meta_signaled
                                            && req
                                                == super::pipelining::DepRequirement::MetadataOnly
                                        {
                                            continue;
                                        }
                                        if let Some(counter) = in_degrees.get(dep_id) {
                                            if counter
                                                .fetch_sub(1, std::sync::atomic::Ordering::SeqCst)
                                                == 1
                                            {
                                                work_queue.push_ready(dep_id.clone());
                                            }
                                        }
                                    }
                                }
                            }
                            work_queue.mark_completed(&unit_id);
                        }
                        Ok(Ok(UnitExecutionStatus::Contested)) => {
                            guard.resolved = true;
                            work_queue.mark_contested(unit_id);
                        }
                        Ok(Err(err)) => {
                            guard.resolved = true;
                            work_queue.mark_failed(format!("{:#}", err));
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
                            work_queue.mark_failed(format!(
                                "Worker thread panicked on unit {}: {}",
                                unit_id, msg
                            ));
                            break;
                        }
                    }
                }
                Ok(())
            }));
        }

        for handle in handles {
            match handle.join() {
                Ok(Err(err)) => {
                    compile_errors.push(format!("{:#}", err));
                }
                Err(panic_err) => {
                    let msg = if let Some(s) = panic_err.downcast_ref::<&str>() {
                        s.to_string()
                    } else if let Some(s) = panic_err.downcast_ref::<String>() {
                        s.clone()
                    } else {
                        "Unknown worker thread panic".to_string()
                    };
                    compile_errors.push(format!("Worker thread panicked: {}", msg));
                }
                Ok(Ok(())) => {}
            }
        }
    });

    if let Some(err) = work_queue.take_error() {
        if !compile_errors.contains(&err) {
            compile_errors.push(err);
        }
    }

    if !compile_errors.is_empty() {
        let combined_error = compile_errors.join("\n");
        let result = ActionResult {
            action_id: "native-dag".to_string(),
            exit_code: 101,
            success: false,
            stdout: String::new(),
            stderr: combined_error,
            cached: false,
        };
        return Ok((
            BuildPlan {
                actions: recorded_actions.lock().unwrap().clone(),
                target_directory: target_dir,
                artifacts: snapshot_map(&completed_artifacts),
                unit_cfgs: snapshot_map(&completed_cfgs),
            },
            result,
        ));
    }

    let all_cached = !*any_missed.lock().unwrap();
    let actions = recorded_actions.lock().unwrap().clone();
    let stderr = compiler_messages.lock().unwrap().join("\n");

    let n_compiled = compiled_actions.load(std::sync::atomic::Ordering::Relaxed);
    let n_cached = cached_actions.load(std::sync::atomic::Ordering::Relaxed);
    let n_total = actions.len();
    eprintln!(
        "[mitts::stats] compiled={} cached={} total={} fast_lock=0",
        n_compiled, n_cached, n_total
    );
    if std::env::var("MITTS_STATS").as_deref() == Ok("json") {
        eprintln!(
            r#"{{"compiled": {}, "cached": {}, "total": {}, "fast_lock": 0}}"#,
            n_compiled, n_cached, n_total
        );
    }

    let result = ActionResult {
        action_id: "native-dag".to_string(),
        exit_code: 0,
        success: true,
        stdout: String::new(),
        stderr,
        cached: all_cached,
    };

    Ok((
        BuildPlan {
            actions,
            target_directory: target_dir,
            artifacts: snapshot_map(&completed_artifacts),
            unit_cfgs: snapshot_map(&completed_cfgs),
        },
        result,
    ))
}

/// Snapshots a thread-safe unit map for post-build consumers (e.g. doctests).
fn snapshot_map<T: Clone>(
    completed: &Mutex<HashMap<String, T>>,
) -> std::collections::BTreeMap<String, T> {
    completed
        .lock()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}
