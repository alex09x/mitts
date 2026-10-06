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
    unit_keys: &HashMap<String, String>,
) -> Result<(BuildPlan, ActionResult)> {
    let cas = match opt.cache_dir {
        Some(ref cd) => ContentAddressedStore::new(cd.join("cas"))?,
        None => ContentAddressedStore::default_store()?,
    };
    let ac = match opt.cache_dir {
        Some(ref cd) => ActionCache::new(cd.join("ac"))?,
        None => ActionCache::default_cache()?,
    };

    let dag = CompilationDag::from_explanation(explanation)?;
    let waves = dag.topological_waves()?;

    let completed_artifacts: Arc<Mutex<HashMap<String, PathBuf>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let recorded_actions: Arc<Mutex<Vec<Action>>> = Arc::new(Mutex::new(Vec::new()));
    let compiler_messages: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let any_missed: Arc<Mutex<bool>> = Arc::new(Mutex::new(false));
    let dep_build_metadata: Arc<Mutex<HashMap<String, HashMap<String, String>>>> =
        Arc::new(Mutex::new(HashMap::new()));

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

    for wave in waves {
        let mut compile_errors: Vec<String> = Vec::new();
        let concurrency = max_concurrency.min(wave.len());

        let (tx, rx) = std::sync::mpsc::channel();
        for unit_id in wave {
            let _ = tx.send(unit_id);
        }
        drop(tx);
        let rx = Arc::new(Mutex::new(rx));

        thread::scope(|s| {
            let mut handles = Vec::new();

            for _ in 0..concurrency {
                let rx = Arc::clone(&rx);
                let cas = cas.clone();
                let ac = ac.clone();
                let completed_artifacts = Arc::clone(&completed_artifacts);
                let recorded_actions = Arc::clone(&recorded_actions);
                let compiler_messages = Arc::clone(&compiler_messages);
                let any_missed = Arc::clone(&any_missed);
                let dep_build_metadata = Arc::clone(&dep_build_metadata);
                let target_dir = target_dir.clone();
                let toolchain = explanation.toolchain.clone();
                let opt = opt.clone();
                let dag = &dag;

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
                    };

                    while let Ok(unit_id) = {
                        let lock = rx.lock().unwrap();
                        lock.recv()
                    } {
                        let Some(node) = dag.get_node(&unit_id) else {
                            continue;
                        };
                        let unit = node.unit.clone();
                        let action_key = unit_keys
                            .get(&unit_id)
                            .cloned()
                            .unwrap_or_else(|| unit_id.clone());

                        execute_unit(&unit, &action_key, &ctx)?;
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

        if !compile_errors.is_empty() {
            let combined_error = compile_errors.join("\n");
            let result = ActionResult {
                action_id: "native-dag".to_string(),
                exit_code: 1,
                success: false,
                stdout: String::new(),
                stderr: combined_error,
                cached: false,
            };
            return Ok((
                BuildPlan {
                    actions: recorded_actions.lock().unwrap().clone(),
                    target_directory: target_dir,
                },
                result,
            ));
        }
    }

    let all_cached = !*any_missed.lock().unwrap();
    let actions = recorded_actions.lock().unwrap().clone();
    let stderr = compiler_messages.lock().unwrap().join("\n");

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
        },
        result,
    ))
}
