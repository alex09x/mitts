/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::planner::explain_plan;
use super::types::{
    BuildProgressEvent, EffectivePlanExplanation, RustAdapterError, RustBuildOptions,
};
use crate::adapter::RustAdapter;
use crate::core::action::{Action, ActionKind, ActionResult, BuildPlan};
use crate::core::build_lock::{
    FastBuildLock, FreshnessCheckArgs, FreshnessResult, RecordStateArgs, RecordStateOptions,
};
use crate::core::build_logger::BuildLogger;
use crate::core::hash::Sha256;
use anyhow::Result;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Computes a deterministic SHA-256 hash of all build-affecting `RustBuildOptions` fields.
pub fn compute_options_hash(
    opt: &RustBuildOptions,
    target_dir: &Path,
    manifest_path: &Path,
) -> String {
    let mut bin_targets = opt.bin_targets.clone();
    bin_targets.sort();
    let mut exclude = opt.exclude.clone();
    exclude.sort();
    let mut test_targets = opt.test_targets.clone();
    test_targets.sort();
    let mut example_targets = opt.example_targets.clone();
    example_targets.sort();
    let mut features = opt.features.clone();
    features.sort();
    let mut packages: Vec<_> = opt.selected_package_names().collect();
    packages.sort_unstable();
    packages.dedup();

    let mut payload = format!(
        "jobs={}|compiler_protocol={}|manifest={}|target_dir={}|target={:?}|crate_types={:?}|start_dir={:?}|release={}|mode={:?}|no_cache={}|native={}|cargo={}|pkgs={:?}|exclude={:?}|bins={:?}|all_bins={}|examples={:?}|lib_only={}|doc_only={}|tests={:?}|all_tests={}|all_benches={}|all_examples={}|features={:?}|all_features={}|no_default_features={}|extra_args={:?}|diag_format={:?}|cache_dir={:?}",
        crate::core::jobs::limit().unwrap_or(0),
        super::compiler_remap::KEY_FLAG,
        manifest_path.display(),
        target_dir.display(),
        opt.target_triple,
        opt.crate_types,
        opt.start_dir,
        opt.release,
        opt.mode,
        opt.no_cache,
        opt.native,
        opt.cargo,
        packages,
        exclude,
        bin_targets,
        opt.all_bins,
        example_targets,
        opt.lib_only,
        opt.doc_only,
        test_targets,
        opt.all_tests,
        opt.all_benches,
        opt.all_examples,
        features,
        opt.all_features,
        opt.no_default_features,
        opt.extra_args,
        opt.effective_diagnostic_format(),
        opt.cache_dir,
    );
    if let Some(profile) = &opt.profile {
        payload.push_str(&format!("|custom_profile={profile}"));
    }
    if !opt.bench_targets.is_empty() {
        let mut benches = opt.bench_targets.clone();
        benches.sort();
        benches.dedup();
        payload.push_str(&format!("|bench_targets={benches:?}"));
    }
    if super::compiler_wrappers::wrapper_cache_enabled() {
        let project_dir = manifest_path.parent().unwrap_or(Path::new("."));
        let config_dir = super::cargo_config::resolve_config_start_dir_with_opt(
            project_dir,
            opt.start_dir.as_deref(),
        );
        if let Ok(ids) = super::compiler_wrappers::resolve_wrapper_identities(&config_dir, true) {
            if let Some(stamp) = super::compiler_wrappers::format_wrapper_identities_stamp(&ids) {
                payload.push_str(&format!("|rustc_wrapper_identity={stamp}"));
            }
        }
        if let Some(dir) = super::wrapper_outputs::declared_dir() {
            payload.push_str(&format!("|rustc_wrapper_outputs={}", dir.display()));
        }
    }
    Sha256::digest(payload.as_bytes())
}

/// Executes the build purely natively with parallel rustc DAG compilation and CAS caching.
pub fn build(opt: &RustBuildOptions) -> Result<(BuildPlan, ActionResult)> {
    crate::core::jobs::limit()?;
    super::profile::set_custom_profile(opt.profile.as_deref());
    super::diagnostic_dedupe::reset();
    super::toolchain_probes::set_cache_root(&crate::core::cache_root::resolve(
        opt.cache_dir.as_deref(),
    ));
    let _manifest_scope = super::manifest_snapshot::ManifestSnapshot::default().enter();
    let manifest_path = RustAdapter::resolve_manifest(opt)?;
    let project_dir = manifest_path.parent().unwrap_or(Path::new("."));
    let config_dir = super::cargo_config::resolve_config_start_dir_with_opt(
        project_dir,
        opt.start_dir.as_deref(),
    );
    let wrappers = crate::adapter::rust::compiler_wrappers::compiler_wrappers(&config_dir, true)?;
    let mut uncached_opt = opt.clone();
    if !wrappers.is_empty() {
        if crate::adapter::rust::compiler_wrappers::wrapper_cache_enabled() {
            // Fail closed if any configured wrapper cannot be resolved or hashed on disk.
            crate::adapter::rust::compiler_wrappers::resolve_wrapper_identities(&config_dir, true)?;
        } else {
            uncached_opt.no_cache = true;
            uncached_opt.fast_lock = Some(false);
            if !opt.no_cache && !opt.quiet {
                eprintln!(
                    "{}",
                    crate::adapter::rust::compiler_wrappers::uncached_wrapper_warning(&wrappers)
                );
            }
        }
    }
    let opt = &uncached_opt;
    let target_dir = RustAdapter::resolve_target_dir(&manifest_path, opt)?;
    let _ = std::fs::create_dir_all(&target_dir);
    let _ = std::fs::write(target_dir.join(".mitts-target"), "mitts-target-v1\n");
    let clean_lock_path = target_dir.join(".mitts-clean.lock");
    let _clean_guard = crate::core::lock::FileLock::acquire_shared(&clean_lock_path).ok();
    let _gc_lease = crate::core::gc::CacheLease::acquire(opt.cache_dir.as_deref());
    let log_path = target_dir.join("build.log");
    let mut logger = BuildLogger::new(Some(log_path));

    let base_profile = match (opt.mode, opt.release) {
        (super::types::BuildMode::Check, true) => "check-release",
        (super::types::BuildMode::Check, false) => "check",
        (super::types::BuildMode::Clippy, true) => "clippy-release",
        (super::types::BuildMode::Clippy, false) => "clippy",
        (super::types::BuildMode::Test, true) => "test-release",
        (super::types::BuildMode::Test, false) => "test",
        (super::types::BuildMode::Build, true) => "release",
        (super::types::BuildMode::Build, false) => "dev",
    };
    // A custom profile keeps a fast lock of its own.
    let profile_label = match &opt.profile {
        Some(custom) => format!("{base_profile}-profile-{custom}"),
        None => base_profile.to_string(),
    };
    let profile = profile_label.as_str();

    // Validate unsupported options explicitly
    for arg in &opt.extra_args {
        if arg.starts_with("--unsupported") {
            return Err(RustAdapterError::UnsupportedOption(arg.clone()).into());
        }
    }

    let toolchain = logger.measure("toolchain", "Detected rustc toolchain", || {
        RustAdapter::detect_toolchain()
    })?;

    let options_hash = compute_options_hash(opt, &target_dir, &manifest_path);

    if !opt.crate_types.is_empty() {
        if !opt.lib_only {
            anyhow::bail!("--crate-type requires --lib in native Mitts builds");
        }
        for crate_type in &opt.crate_types {
            if !matches!(
                crate_type.as_str(),
                "lib" | "rlib" | "dylib" | "cdylib" | "staticlib" | "proc-macro"
            ) {
                anyhow::bail!("unsupported --crate-type `{crate_type}`");
            }
        }
    }
    if opt.target_triple.is_some()
        && opt
            .extra_args
            .iter()
            .any(|arg| arg == "--target" || arg.starts_with("--target="))
    {
        anyhow::bail!("pass --target before `--`; do not also forward a rustc --target option");
    }

    // A fast-lock hit must not bypass the lockfile contract selected by this invocation.
    if opt.locked || opt.frozen {
        let lock_path = project_dir
            .ancestors()
            .map(|path| path.join("Cargo.lock"))
            .find(|path| path.is_file())
            .ok_or_else(|| {
                anyhow::anyhow!("Cargo.lock is missing and --locked/--frozen was specified")
            })?;
        let lock = super::lockfile::CargoLock::from_file(&lock_path)?;
        if !lock.validate_manifest_compatibility(&manifest_path)? {
            anyhow::bail!(
                "Cargo.lock is incompatible with the manifest and --locked/--frozen was specified"
            );
        }
    }

    // Fast build lock pre-flight check: if zero project files changed, return in microsecond time.
    // Strictly restricted to Build and Check modes. In Test mode, test execution cannot be
    // cached by fast-lock because tests have runtime inputs and assertions that must be executed.
    if !opt.no_cache
        && fast_lock_enabled(opt)
        && matches!(
            opt.mode,
            super::types::BuildMode::Build | super::types::BuildMode::Check
        )
    {
        let lock_start = std::time::Instant::now();
        let (freshness, root_env, root_link_args) =
            FastBuildLock::check_freshness_with_runtime_context(
                &FreshnessCheckArgs {
                    project_dir,
                    profile,
                    package: &opt.package,
                    features: &opt.features,
                    no_default_features: opt.no_default_features,
                    all_features: opt.all_features,
                    toolchain_version: &toolchain.version,
                },
                Some(&target_dir),
                Some(&options_hash),
                true,
            );

        match freshness {
            FreshnessResult::Fresh {
                action_count,
                root_binary,
                stderr,
            } => {
                let dur = lock_start.elapsed();
                logger.record_phase(
                    "lock_check",
                    dur,
                    &format!("Fast build lock HIT: {} actions up-to-date", action_count),
                );
                let _ = logger.write_trace(profile, 0, "Fast lock hit (0ms no-op)");
                if !opt.quiet {
                    eprintln!(
                        "[mitts::stats] compiled=0 cached=0 tainted=0 total={} fast_lock=1",
                        action_count
                    );
                }
                if std::env::var("MITTS_STATS").as_deref() == Ok("json") {
                    eprintln!(
                        r#"{{"compiled": 0, "cached": 0, "total": {}, "fast_lock": 1}}"#,
                        action_count
                    );
                }

                if let Some(ref ref_stderr) = stderr {
                    if !ref_stderr.is_empty() {
                        if let Some(ref cb) = opt.progress {
                            cb.0(BuildProgressEvent::CompilerMessage(ref_stderr.clone()));
                        }
                    }
                }

                let mut actions = Vec::new();
                if let Some(ref bin) = root_binary {
                    actions.push(Action {
                        id: "root-bin".to_string(),
                        kind: ActionKind::CompileBin,
                        description: format!("Existing bin {}", bin.display()),
                        toolchain: toolchain.clone(),
                        inputs: Vec::new(),
                        outputs: vec![bin.clone()],
                        args: Vec::new(),
                        env: root_env,
                        dependencies: Vec::new(),
                    });
                }
                let mut unit_link_args = BTreeMap::new();
                if !root_link_args.is_empty() {
                    unit_link_args.insert("root-bin".to_string(), root_link_args);
                }
                let plan = BuildPlan {
                    actions,
                    target_directory: target_dir,
                    artifacts: Default::default(),
                    unit_cfgs: Default::default(),
                    unit_link_args,
                    ..Default::default()
                };
                let result = ActionResult {
                    action_id: "fast-lock-hit".to_string(),
                    exit_code: 0,
                    success: true,
                    stdout: String::new(),
                    stderr: stderr.unwrap_or_default(),
                    cached: true,
                };
                return Ok((plan, result));
            }
            FreshnessResult::Stale(ref reason) => {
                let dur = lock_start.elapsed();
                logger.record_phase("lock_check", dur, &format!("Lock miss / stale: {}", reason));
            }
        }
    }

    let is_verbose = !opt.quiet && (opt.verbose > 0 || std::env::var("MITTS_VERBOSE").is_ok());
    if is_verbose {
        println!(
            "[mitts::plan] Planning compilation from {}",
            manifest_path.display()
        );
    }

    let explanation = logger.measure("plan", "native manifest & lockfile planning", || {
        explain_plan(opt)
    })?;

    if is_verbose {
        println!(
            "[mitts::plan] Planned {} units. Deriving action keys...",
            explanation.units.len()
        );
    }

    let key_deriver = logger.measure("keys", "Action key derivation and input hashing", || {
        super::key::derive_unit_action_key_context(opt, &explanation)
    })?;
    let unit_keys = (*key_deriver.initial_base_keys).clone();

    if is_verbose {
        println!(
            "[mitts::dag] Starting parallel DAG execution for {} units...",
            unit_keys.len()
        );
    }

    let (plan, result) =
        logger.measure("compile", "native parallel rustc DAG execution", || {
            super::dag_runner::run_native_dag_build(opt, &explanation, &key_deriver)
        })?;

    record_state_and_trace(
        project_dir,
        &target_dir,
        &options_hash,
        profile,
        opt,
        &explanation,
        &toolchain,
        &plan,
        &result,
        &mut logger,
    );

    Ok((plan, result))
}

#[allow(clippy::too_many_arguments)]
fn record_state_and_trace(
    project_dir: &Path,
    target_dir: &Path,
    options_hash: &str,
    profile: &str,
    opt: &RustBuildOptions,
    explanation: &EffectivePlanExplanation,
    toolchain: &crate::core::action::ToolchainInfo,
    plan: &BuildPlan,
    result: &ActionResult,
    logger: &mut BuildLogger,
) {
    if result.success
        && !opt.no_cache
        && fast_lock_enabled(opt)
        && matches!(
            opt.mode,
            super::types::BuildMode::Build | super::types::BuildMode::Check
        )
    {
        let mut outputs = Vec::new();
        let mut bin_candidates = Vec::new();
        for action in &plan.actions {
            for out in &action.outputs {
                outputs.push(out.clone());
            }
            if action.kind == ActionKind::CompileBin {
                if let Some(first) = action.outputs.first() {
                    bin_candidates.push(first.clone());
                }
            }
        }
        bin_candidates.sort();
        let root_binary = if let Some(req_bin) = opt.bin_targets.first() {
            bin_candidates
                .iter()
                .find(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.trim_end_matches(".exe") == req_bin)
                })
                .cloned()
                .or_else(|| bin_candidates.into_iter().next())
        } else {
            bin_candidates.into_iter().next()
        };
        let mut extra_pkg_dirs: Vec<PathBuf> = Vec::new();
        let ws_root = super::compiler_resolve::find_workspace_root(&explanation.manifest_path);
        if ws_root != project_dir {
            extra_pkg_dirs.push(ws_root);
        }
        for unit in &explanation.units {
            if super::unit_inputs::is_mutable_unit(unit) {
                let pkg_dir = super::key_flags::resolve_unit_pkg_dir(unit);
                if pkg_dir != project_dir && !extra_pkg_dirs.contains(&pkg_dir) {
                    extra_pkg_dirs.push(pkg_dir);
                }
            }
        }

        let root_binary_env = root_binary.as_ref().and_then(|rb| {
            plan.actions
                .iter()
                .find(|a| a.outputs.contains(rb))
                .map(|a| &a.env)
        });
        let mut root_binary_link_args: Vec<String> = plan
            .unit_link_args
            .values()
            .flat_map(|v| v.iter().cloned())
            .collect();
        for art in plan.artifacts.values() {
            if let Some(parent) = art.parent() {
                root_binary_link_args.push(format!("-L{}", parent.display()));
            }
        }
        if opt.native && !plan.artifacts.is_empty() {
            let cas_artifacts = crate::core::cache_root::resolve(opt.cache_dir.as_deref())
                .join("cas")
                .join("artifacts");
            root_binary_link_args.push(format!("-L{}", cas_artifacts.display()));
        }

        let _ = FastBuildLock::record_state_with_options(
            &RecordStateArgs {
                project_dir,
                profile,
                package: &opt.package,
                features: &opt.features,
                no_default_features: opt.no_default_features,
                all_features: opt.all_features,
                toolchain_version: &toolchain.version,
                action_count: plan.actions.len(),
                root_binary,
                outputs: &outputs,
                stderr: if result.stderr.is_empty() {
                    None
                } else {
                    Some(&result.stderr)
                },
            },
            &RecordStateOptions {
                target_dir: Some(target_dir),
                options_hash: Some(options_hash),
                extra_package_dirs: &extra_pkg_dirs,
                tracked_extra_files: Some(&plan.tracked_extra_files),
                tracked_env_vars: Some(&plan.tracked_env_vars),
                has_complete_inputs: !plan.inputs_incomplete,
                root_binary_env,
                root_binary_link_args: &root_binary_link_args,
            },
        );
    }
    let outcome = if result.success {
        "Build completed successfully"
    } else {
        "Build failed"
    };
    let _ = logger.write_trace(profile, result.exit_code, outcome);
}

/// The fast build lock is on by default only for the default cache root: an isolated
/// `--cache-dir` (tests, benchmarks) opts out unless `RustBuildOptions::fast_lock` or
/// `MITTS_FAST_LOCK=1` says otherwise.
fn fast_lock_enabled(opt: &RustBuildOptions) -> bool {
    // The lock verifies only target/ outputs; a rustc wrapper's declared outputs live
    // elsewhere and must be restored by the DAG on every build (issue 91).
    if super::wrapper_outputs::declared_dir().is_some() {
        return false;
    }
    opt.fast_lock
        .or_else(|| {
            std::env::var("MITTS_FAST_LOCK")
                .ok()
                .map(|v| !matches!(v.trim(), "" | "0" | "false"))
        })
        .unwrap_or(opt.cache_dir.is_none())
}
