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
use super::dag_scheduler::WaveWorkQueue;
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

    let completed_artifacts = Arc::new(Mutex::new(HashMap::<String, PathBuf>::new()));
    let completed_cfgs = Arc::new(Mutex::new(HashMap::<String, Vec<String>>::new()));
    let completed_env = Arc::new(Mutex::new(HashMap::<String, HashMap<String, String>>::new()));
    let completed_link_args = Arc::new(Mutex::new(HashMap::<String, Vec<String>>::new()));
    let recorded_actions = Arc::new(Mutex::new(Vec::<Action>::new()));
    let compiler_messages = Arc::new(Mutex::new(Vec::<String>::new()));
    let any_missed = Arc::new(Mutex::new(false));
    let dep_build_metadata =
        Arc::new(Mutex::new(HashMap::<String, HashMap<String, String>>::new()));
    let resolved_keys = Arc::new(Mutex::new(HashMap::<String, String>::new()));
    let tracked_extra_files = Arc::new(Mutex::new(
        std::collections::BTreeMap::<PathBuf, String>::new(),
    ));
    let mut initial_env_vars = std::collections::BTreeMap::new();
    for unit in &explanation.units {
        for var in &unit.env_vars {
            if var == "OUT_DIR"
                || var == "CARGO_MANIFEST_DIR"
                || var == "CARGO_CRATE_NAME"
                || var == "CARGO_BIN_NAME"
                || var.starts_with("CARGO_PKG_")
            {
                continue;
            }
            initial_env_vars.insert(var.clone(), std::env::var(var).ok());
        }
    }
    let tracked_env_vars = Arc::new(Mutex::new(initial_env_vars));

    let target_dir = explanation.target_directory.clone();
    fs::create_dir_all(&target_dir)?;

    let profile_name = if opt.release { "release" } else { "debug" };
    let target_index = Arc::new(Mutex::new(crate::core::TargetArtifactIndex::load(
        &target_dir,
        profile_name,
    )));

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

    let shared = super::dag_worker::WorkerShared {
        dag: &dag,
        work_queue: &work_queue,
        in_degrees: &in_degrees,
        metadata_signaled: &metadata_signaled,
        resolved_keys: &resolved_keys,
        unit_keys,
        deriver,
    };

    thread::scope(|s| {
        let mut handles = Vec::new();

        for _ in 0..concurrency {
            let shared = &shared;
            let on_metadata_ready = &on_metadata_ready;
            let cas = cas.clone();
            let ac = ac.clone();
            let completed_artifacts = Arc::clone(&completed_artifacts);
            let completed_cfgs = Arc::clone(&completed_cfgs);
            let completed_env = Arc::clone(&completed_env);
            let completed_link_args = Arc::clone(&completed_link_args);
            let recorded_actions = Arc::clone(&recorded_actions);
            let compiler_messages = Arc::clone(&compiler_messages);
            let any_missed = Arc::clone(&any_missed);
            let dep_build_metadata = Arc::clone(&dep_build_metadata);
            let resolved_keys = Arc::clone(&resolved_keys);
            let jobserver = jobserver.clone();
            let is_first_job = Arc::clone(&is_first_job);
            let target_dir = target_dir.clone();
            let toolchain = explanation.toolchain.clone();
            let opt = opt.clone();
            let compiled_actions = Arc::clone(&compiled_actions);
            let cached_actions = Arc::clone(&cached_actions);
            let target_index = Arc::clone(&target_index);
            let tracked_extra_files = Arc::clone(&tracked_extra_files);
            let tracked_env_vars = Arc::clone(&tracked_env_vars);

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
                    completed_env: Some(&completed_env),
                    completed_link_args: Some(&completed_link_args),
                    target_index: Some(&target_index),
                    tracked_extra_files: Some(&tracked_extra_files),
                    tracked_env_vars: Some(&tracked_env_vars),
                };

                super::dag_worker::run_worker_loop(shared, &ctx)
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

    let _ = target_index.lock().unwrap().save(&target_dir, profile_name);

    if let Some(err) = work_queue.take_error() {
        if !compile_errors.contains(&err) {
            compile_errors.push(err);
        }
    }
    if compile_errors.is_empty() {
        let (done, total) = work_queue.completed_and_total();
        if done < total {
            compile_errors.push(format!(
                "DAG execution stalled: only {done} of {total} compilation units completed"
            ));
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
                unit_env: snapshot_map(&completed_env),
                unit_link_args: snapshot_map(&completed_link_args),
                tracked_extra_files: tracked_extra_files.lock().unwrap().clone(),
                tracked_env_vars: tracked_env_vars.lock().unwrap().clone(),
                inputs_incomplete: true,
            },
            result,
        ));
    }

    let all_cached = !*any_missed.lock().unwrap();
    let actions = recorded_actions.lock().unwrap().clone();
    let stderr = compiler_messages.lock().unwrap().join("\n");

    let n_compiled = compiled_actions.load(std::sync::atomic::Ordering::Relaxed);
    let n_cached = cached_actions.load(std::sync::atomic::Ordering::Relaxed);
    let n_tainted = resolved_keys
        .lock()
        .unwrap()
        .values()
        .filter(|k| k.contains(":unhashable:"))
        .count();
    let n_total = actions.len();
    eprintln!(
        "[mitts::stats] compiled={} cached={} tainted={} total={} fast_lock=0",
        n_compiled, n_cached, n_tainted, n_total
    );
    if std::env::var("MITTS_STATS").as_deref() == Ok("json") {
        eprintln!(
            r#"{{"compiled": {}, "cached": {}, "tainted": {}, "total": {}, "fast_lock": 0}}"#,
            n_compiled, n_cached, n_tainted, n_total
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

    let extra_files_snap = tracked_extra_files.lock().unwrap().clone();
    let env_vars_snap = tracked_env_vars.lock().unwrap().clone();
    let has_untracked_env = explanation
        .units
        .iter()
        .any(|u| u.cache_eligibility.contains("untracked dynamic env"));

    Ok((
        BuildPlan {
            actions,
            target_directory: target_dir,
            artifacts: snapshot_map(&completed_artifacts),
            unit_cfgs: snapshot_map(&completed_cfgs),
            unit_env: snapshot_map(&completed_env),
            unit_link_args: snapshot_map(&completed_link_args),
            tracked_extra_files: extra_files_snap,
            tracked_env_vars: env_vars_snap,
            inputs_incomplete: n_tainted > 0 || has_untracked_env,
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
