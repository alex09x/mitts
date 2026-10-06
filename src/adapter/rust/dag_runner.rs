/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::compiler::{execute_build_script, RustcInvocation};
use super::dag::CompilationDag;
use super::types::{
    BuildProgressEvent, EffectivePlanExplanation, PlannedCompilationUnit, RustBuildOptions,
};
use crate::core::action::{Action, ActionKind, ActionResult, BuildPlan, ToolchainInfo};
use crate::core::action_cache::{ActionCache, ActionCacheEntry, CachedOutput};
use crate::core::cas::ContentAddressedStore;
use crate::core::lock::FileLock;
use anyhow::{bail, Result};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
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

    let target_dir = explanation.target_directory.clone();
    fs::create_dir_all(&target_dir)?;

    let is_cache_enabled = !opt.no_cache;

    for wave in waves {
        let mut compile_errors: Vec<String> = Vec::new();

        thread::scope(|s| {
            let mut handles = Vec::new();

            for unit_id in wave {
                let unit = dag.get_node(&unit_id).unwrap().unit.clone();
                let action_key = unit_keys
                    .get(&unit_id)
                    .cloned()
                    .unwrap_or_else(|| unit_id.clone());

                let cas = cas.clone();
                let ac = ac.clone();
                let completed_artifacts = Arc::clone(&completed_artifacts);
                let recorded_actions = Arc::clone(&recorded_actions);
                let compiler_messages = Arc::clone(&compiler_messages);
                let any_missed = Arc::clone(&any_missed);
                let target_dir = target_dir.clone();
                let toolchain = explanation.toolchain.clone();
                let opt = opt.clone();

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
                    };
                    execute_unit(&unit, &action_key, &ctx)
                }));
            }

            for handle in handles {
                if let Ok(Err(err)) = handle.join() {
                    compile_errors.push(format!("{:#}", err));
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

struct ExecutionContext<'a> {
    opt: &'a RustBuildOptions,
    toolchain: &'a ToolchainInfo,
    target_dir: &'a Path,
    cas: &'a ContentAddressedStore,
    ac: &'a ActionCache,
    is_cache_enabled: bool,
    completed_artifacts: &'a Arc<Mutex<HashMap<String, PathBuf>>>,
    recorded_actions: &'a Arc<Mutex<Vec<Action>>>,
    compiler_messages: &'a Arc<Mutex<Vec<String>>>,
    any_missed: &'a Arc<Mutex<bool>>,
}

fn execute_unit(
    unit: &PlannedCompilationUnit,
    action_key: &str,
    ctx: &ExecutionContext,
) -> Result<()> {
    let clean_crate = unit.target_name.replace('-', "_");
    let is_bin = unit.kind == "bin";

    // 1. Check Action Cache (read phase)
    if ctx.is_cache_enabled {
        if let Ok(Some(entry)) = ctx.ac.get(action_key) {
            let mut valid = !entry.outputs.is_empty();
            for out in &entry.outputs {
                if !ctx.cas.has_object(&out.cas_hash) || !ctx.cas.verify_object(&out.cas_hash).unwrap_or(false) {
                    valid = false;
                    break;
                }
            }

            if valid {
                if is_bin {
                    let profile_name = if ctx.opt.release { "release" } else { "debug" };
                    let dest_bin = ctx.target_dir.join(profile_name).join(&unit.target_name);
                    for out in &entry.outputs {
                        if out.executable {
                            let rel_dest = ctx.target_dir.join(&out.relative_path);
                            ctx.cas.restore_object(&out.cas_hash, &rel_dest, true)?;
                            if rel_dest != dest_bin {
                                if let Some(parent) = dest_bin.parent() {
                                    let _ = fs::create_dir_all(parent);
                                }
                                let _ = fs::copy(&rel_dest, &dest_bin);
                            }
                            break;
                        }
                    }
                } else {
                    for out in &entry.outputs {
                        let ext = out.relative_path
                            .extension()
                            .and_then(|e| e.to_str())
                            .unwrap_or("rlib");
                        if ext == "rlib" || ext == "dylib" {
                            let named = ctx.cas.named_artifact_path(&unit.target_name, &out.cas_hash, ext)?;
                            ctx.completed_artifacts
                                .lock()
                                .unwrap()
                                .insert(unit.unit_id(), named);
                            break;
                        }
                    }
                }

                ctx.recorded_actions.lock().unwrap().push(Action {
                    id: entry.action_id.clone(),
                    kind: if is_bin {
                        ActionKind::CompileBin
                    } else {
                        ActionKind::CompileRlib
                    },
                    description: format!("Restored {} from CAS", entry.action_id),
                    toolchain: ctx.toolchain.clone(),
                    inputs: Vec::new(),
                    outputs: entry.outputs.iter().map(|o| ctx.target_dir.join(&o.relative_path)).collect(),
                    args: Vec::new(),
                    env: HashMap::new(),
                    dependencies: Vec::new(),
                });

                return Ok(());
            }
        }
    }

    *ctx.any_missed.lock().unwrap() = true;

    if let Some(ref cb) = ctx.opt.progress {
        cb.0(BuildProgressEvent::Compiling(format!("{} v{}", unit.package_name, unit.version)));
    }

    // 2. Single-flight lock to eliminate duplicate work across processes
    let lock_path = ctx.ac.lock_path(action_key);
    let _lock = FileLock::acquire_exclusive(&lock_path)?;

    // Staging directory for build isolation
    let pid = std::process::id();
    let thread_id = format!("{:?}", thread::current().id());
    let clean_tid: String = thread_id.chars().filter(|c| c.is_alphanumeric()).collect();
    let staging_dir = ctx.cas.root().join("tmp").join(format!("stage_{}_{}_{}", clean_crate, pid, clean_tid));
    fs::create_dir_all(&staging_dir)?;

    // Gather dependencies' CAS artifact paths for --extern
    let mut extern_crates = Vec::new();
    {
        let completed = ctx.completed_artifacts.lock().unwrap();
        for dep_id in &unit.dependencies {
            if let Some(artifact_path) = completed.get(dep_id) {
                let dep_pkg = dep_id.split("::").next().unwrap_or(dep_id);
                let dep_crate = if let Some(rest) = dep_id.rsplit(':').next() {
                    rest
                } else {
                    dep_pkg
                };
                extern_crates.push((dep_crate.to_string(), artifact_path.clone()));
            }
        }
    }

    // Sandboxed build.rs execution if present
    let directives = execute_build_script(unit, &staging_dir)?;

    let profile_name = if ctx.opt.release { "release" } else { "debug" };
    let profile_dir = ctx.target_dir.join(profile_name);
    fs::create_dir_all(&profile_dir)?;

    let dest_binary = if is_bin {
        Some(profile_dir.join(&unit.target_name))
    } else {
        None
    };

    let invocation = RustcInvocation {
        unit,
        action_key,
        out_dir: &staging_dir,
        dest_binary: dest_binary.as_deref(),
        extern_crates: &extern_crates,
        release: ctx.opt.release,
        features: &ctx.opt.features,
        directives: Some(&directives),
    };

    let output = invocation.execute()?;

    if !output.stdout.is_empty() {
        ctx.compiler_messages.lock().unwrap().push(output.stdout);
    }
    if !output.stderr.is_empty() {
        ctx.compiler_messages.lock().unwrap().push(output.stderr.clone());
    }

    if !output.success {
        let _ = fs::remove_dir_all(&staging_dir);
        bail!("rustc failed for unit '{}' (exit code {}):\n{}", unit.unit_id(), output.exit_code, output.stderr);
    }

    let mut cached_outputs = Vec::new();
    let mut action_outputs = Vec::new();

    if is_bin {
        if let Some(bin_path) = dest_binary {
            let (hash, size) = ctx.cas.store_file(&bin_path)?;
            let rel_path = PathBuf::from(format!("{}/{}", profile_name, unit.target_name));
            cached_outputs.push(CachedOutput {
                relative_path: rel_path,
                cas_hash: hash,
                file_size: size,
                executable: true,
            });
            let alt_path = profile_dir.join(&clean_crate);
            if alt_path != bin_path {
                let _ = fs::copy(&bin_path, &alt_path);
            }
            action_outputs.push(bin_path);
        }
    } else {
        // Publish primary rlib/dylib to CAS
        if let Some(ref lib_path) = output.primary_output {
            let (hash, size) = ctx.cas.store_file(lib_path)?;
            let ext = lib_path.extension().and_then(|e| e.to_str()).unwrap_or("rlib");
            let named = ctx.cas.named_artifact_path(&unit.target_name, &hash, ext)?;

            cached_outputs.push(CachedOutput {
                relative_path: PathBuf::from(format!("lib{}.{}", clean_crate, ext)),
                cas_hash: hash.clone(),
                file_size: size,
                executable: false,
            });
            ctx.completed_artifacts
                .lock()
                .unwrap()
                .insert(unit.unit_id(), named.clone());
            action_outputs.push(named);
        }

        // Publish rmeta if pipelined
        if let Some(ref rmeta_path) = output.metadata_output {
            let (meta_hash, meta_size) = ctx.cas.store_file(rmeta_path)?;
            let _ = ctx.cas.named_artifact_path(&unit.target_name, &meta_hash, "rmeta");
            cached_outputs.push(CachedOutput {
                relative_path: PathBuf::from(format!("lib{}.rmeta", clean_crate)),
                cas_hash: meta_hash,
                file_size: meta_size,
                executable: false,
            });
        }
    }

    // Clean up staging directory immediately to guarantee zero workspace target pollution
    let _ = fs::remove_dir_all(&staging_dir);

    // Record in Action Cache
    if ctx.is_cache_enabled && !cached_outputs.is_empty() {
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let entry = ActionCacheEntry {
            action_key: action_key.to_string(),
            action_id: unit.unit_id(),
            created_at_secs: now_secs,
            outputs: cached_outputs,
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
        };
        let _ = ctx.ac.put(&entry);
    }

    ctx.recorded_actions.lock().unwrap().push(Action {
        id: unit.unit_id(),
        kind: if is_bin {
            ActionKind::CompileBin
        } else {
            ActionKind::CompileRlib
        },
        description: format!("Compiled {}", unit.unit_id()),
        toolchain: ctx.toolchain.clone(),
        inputs: Vec::new(),
        outputs: action_outputs,
        args: Vec::new(),
        env: HashMap::new(),
        dependencies: unit.dependencies.clone(),
    });

    Ok(())
}
