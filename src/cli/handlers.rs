/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::adapter::{FetchOptions, RustAdapter, RustBuildOptions};
use crate::cli::{CleanArgs, ExplainArgs, FetchArgs, GcArgs, Ui};
use crate::core::build_lock::FastBuildLock;
use crate::core::{ActionCache, CacheGc, ContentAddressedStore, GcOptions, SourceStore};

pub fn handle_explain(args: &ExplainArgs) -> i32 {
    let opt = RustBuildOptions {
        manifest_path: args.manifest_path.clone(),
        package: args.package.clone(),
        release: args.release,
        features: args.features.clone(),
        no_default_features: args.no_default_features,
        all_features: args.all_features,
        ..Default::default()
    };

    let explanation = match RustAdapter::explain_plan(&opt) {
        Ok(exp) => exp,
        Err(err) => {
            Ui::report_error(&err);
            return 1;
        }
    };

    if let Some(ref keys_path) = args.keys {
        let keys_res = crate::adapter::rust::key::derive_unit_action_keys(&opt, &explanation)
            .and_then(|map| {
                let btree: std::collections::BTreeMap<String, String> = map.into_iter().collect();
                let json = serde_json::to_string_pretty(&btree)?;
                std::fs::write(keys_path, json)?;
                Ok(())
            });
        if let Err(err) = keys_res {
            Ui::report_error(&err);
            return 1;
        }
    }

    if args.json {
        match serde_json::to_string_pretty(&explanation) {
            Ok(json_str) => {
                println!("{json_str}");
                return 0;
            }
            Err(err) => {
                eprintln!("Failed to serialize explanation to JSON: {err}");
                return 1;
            }
        }
    }

    let project_dir = explanation.manifest_path.parent();
    println!("Mitts Build Plan Explanation\n============================");
    println!("Manifest:         {}", explanation.manifest_path.display());
    println!(
        "Target Directory: {}",
        explanation.target_directory.display()
    );
    println!("Profile:          {}", explanation.profile);
    println!(
        "Toolchain:        {} ({})",
        explanation.toolchain.name, explanation.toolchain.version
    );
    if let Some(ref target) = explanation.toolchain.target_triple {
        println!("Host Target:      {}", target);
    }
    println!(
        "\nEffective Compilation Units ({} total):",
        explanation.units.len()
    );
    for (i, unit) in explanation.units.iter().enumerate() {
        println!(
            "  {}. {} v{} [{}: {}]",
            i + 1,
            unit.package_name,
            unit.version,
            unit.kind,
            unit.target_name
        );
        let display_src = project_dir
            .and_then(|p| unit.src_path.strip_prefix(p).ok())
            .unwrap_or(&unit.src_path);
        println!("     Source:      {}", display_src.display());
        if !unit.features.is_empty() {
            println!("     Features:    {}", unit.features.join(", "));
        }
        if !unit.dependencies.is_empty() {
            println!("     Deps:        {}", unit.dependencies.join(", "));
        }
        println!("     Eligibility: {}", unit.cache_eligibility);
    }
    println!("\nActive Milestone: M3 (Fast parallel fetcher & Cargo.lock resolution)");
    println!("Cache Policy:     Immutable CAS, Action Cache, and Global Source Store");
    println!("Coexistence:      Ordinary Cargo target/ and rust-analyzer untouched");
    0
}

pub fn handle_fetch(args: &FetchArgs) -> i32 {
    let opt = RustBuildOptions {
        manifest_path: args.manifest_path.clone(),
        ..Default::default()
    };
    let manifest_path = match RustAdapter::resolve_manifest(&opt) {
        Ok(p) => p,
        Err(err) => {
            Ui::report_error(&err);
            return 1;
        }
    };

    let store_res = args
        .cache_dir
        .as_ref()
        .map_or_else(SourceStore::default_store, |cd| {
            SourceStore::new(cd.join("sources"))
        });
    let store = match store_res {
        Ok(s) => s,
        Err(e) => {
            Ui::report_error(&e);
            return 1;
        }
    };

    let fetch_opt = FetchOptions {
        offline: args.offline,
        locked: args.locked,
        frozen: args.frozen,
    };

    Ui::header("fetch");
    Ui::stage_start(
        "🔍",
        &format!("Resolving dependencies for {}", manifest_path.display()),
    );

    match RustAdapter::fetch(&manifest_path, &store, &fetch_opt) {
        Ok(result) => {
            Ui::stage_step("📦", "Synchronizing packages with registry");
            let cached_msg = format!(
                "{} packages already present in registry",
                result.already_cached
            );
            Ui::stage_item(false, &Ui::badge_cached(), &cached_msg);
            let fetched_msg = format!(
                "{} packages downloaded & verified (parallel)",
                result.fetched
            );
            Ui::stage_item(true, &Ui::badge_download(), &fetched_msg);
            Ui::finish(
                "sources",
                &format!("{}ms", result.duration_ms),
                &format!("registry: {}", store.root().display()),
            );
            0
        }
        Err(err) => {
            Ui::report_error(&err);
            1
        }
    }
}

fn open_cas_and_ac(
    cache_dir: Option<&std::path::PathBuf>,
) -> anyhow::Result<(ContentAddressedStore, ActionCache)> {
    let cas = cache_dir.map_or_else(ContentAddressedStore::default_store, |cd| {
        ContentAddressedStore::new(cd.join("cas"))
    })?;
    let ac = cache_dir.map_or_else(ActionCache::default_cache, |cd| {
        ActionCache::new(cd.join("ac"))
    })?;
    Ok((cas, ac))
}

pub fn handle_gc(args: &GcArgs) -> i32 {
    let (cas, ac) = match open_cas_and_ac(args.cache_dir.as_ref()) {
        Ok(pair) => pair,
        Err(e) => {
            Ui::report_error(&e);
            return 1;
        }
    };
    let opts = GcOptions {
        max_bytes: args.max_bytes,
        max_age: args.max_age_secs.map(std::time::Duration::from_secs),
    };
    match CacheGc::new(&cas, &ac).prune(&opts) {
        Ok(report) if report.skipped_active_build => {
            println!(
                "mitts: GC skipped — active build lease held ({} bytes retained)",
                report.bytes_retained
            );
            0
        }
        Ok(report) => {
            println!(
                "mitts: GC completed — {} examined, {} pruned ({} bytes freed, {} bytes retained)",
                report.objects_examined,
                report.objects_pruned,
                report.bytes_freed,
                report.bytes_retained
            );
            0
        }
        Err(err) => {
            Ui::report_error(&err);
            1
        }
    }
}

pub fn handle_clean(args: &CleanArgs) -> i32 {
    let opt = RustBuildOptions {
        manifest_path: args.manifest_path.clone(),
        target_dir: args.target_dir.clone(),
        ..Default::default()
    };
    let manifest_path = match RustAdapter::resolve_manifest(&opt) {
        Ok(p) => p,
        Err(err) => {
            Ui::report_error(&err);
            return 1;
        }
    };
    let project_dir = manifest_path.parent().unwrap_or(std::path::Path::new("."));
    let mitts_dir = match RustAdapter::resolve_target_dir(&manifest_path, &opt) {
        Ok(d) => d,
        Err(err) => {
            Ui::report_error(&err);
            return 1;
        }
    };
    let ws_root = crate::adapter::rust::compiler_resolve::find_workspace_root(&manifest_path);
    let mitts_abs = if mitts_dir.is_absolute() {
        mitts_dir.clone()
    } else {
        std::env::current_dir().unwrap_or_default().join(&mitts_dir)
    };
    let mitts_c = mitts_abs.canonicalize().unwrap_or(mitts_abs);
    let proj_c = project_dir
        .canonicalize()
        .unwrap_or_else(|_| project_dir.to_path_buf());
    let ws_c = ws_root.canonicalize().unwrap_or(ws_root);
    let home_c = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .map(|p| p.canonicalize().unwrap_or(p));
    if mitts_c == std::path::Path::new("/")
        || home_c.as_ref().is_some_and(|h| mitts_c == *h)
        || proj_c.starts_with(&mitts_c)
        || ws_c.starts_with(&mitts_c)
    {
        eprintln!(
            "mitts error: refusing to clean unsafe target directory: {}",
            mitts_dir.display()
        );
        return 1;
    }
    if mitts_dir.exists() {
        let has_marker = mitts_dir.join(".mitts-target").is_file()
            || (args.target_dir.is_none()
                && (mitts_dir.join("build.log").is_file()
                    || mitts_dir.join("build-dev.lock").is_file()
                    || mitts_dir.join("build-release.lock").is_file()));
        if !has_marker {
            eprintln!(
                "mitts error: refusing to clean unowned directory without Mitts marker (.mitts-target): {}",
                mitts_dir.display()
            );
            return 1;
        }
    }
    let legacy_mitts_dir = project_dir.join(".mitts");

    Ui::header("clean");
    let clean_lock_path = mitts_dir
        .parent()
        .unwrap_or(&mitts_dir)
        .join(".mitts-clean.lock");
    let _clean_guard = crate::core::lock::FileLock::acquire_exclusive(&clean_lock_path).ok();

    let profiles: &[&str] = if args.release {
        &["release", "check-release", "clippy-release", "test-release"]
    } else {
        &[
            "dev",
            "release",
            "check",
            "check-release",
            "clippy",
            "clippy-release",
            "test",
            "test-release",
        ]
    };
    if args.release && mitts_dir.exists() {
        let _ = std::fs::write(mitts_dir.join(".mitts-target"), "mitts-target-v1\n");
    }
    for profile in profiles {
        let _ = std::fs::remove_file(FastBuildLock::lock_path_in_target(&mitts_dir, profile));
    }
    let badge = Ui::badge_cached();
    if args.release {
        Ui::stage_item(true, &badge, "Removed fast build lock (release)");
    } else {
        let _ = std::fs::remove_file(legacy_mitts_dir.join("build.lock"));
        Ui::stage_item(true, &badge, "Removed fast build locks");
    }
    let _ = std::fs::remove_file(mitts_dir.join("build.log"));
    let _ = std::fs::remove_file(legacy_mitts_dir.join("build.log"));

    if args.release {
        let rel_dir = mitts_dir.join("release");
        if rel_dir.exists() {
            let _ = std::fs::remove_dir_all(&rel_dir);
            Ui::stage_item(true, &badge, "Removed target/mitts/release directory");
        }
    } else {
        if mitts_dir.exists() {
            let _ = std::fs::remove_dir_all(&mitts_dir);
            Ui::stage_item(true, &badge, "Removed target/mitts directory");
        }
        if legacy_mitts_dir.exists() {
            let _ = std::fs::remove_dir_all(&legacy_mitts_dir);
        }
    }

    if mitts_dir.exists() {
        clean_stray_tmp_links(&mitts_dir);
    }

    if args.cache {
        if let Ok((cas, ac)) = open_cas_and_ac(args.cache_dir.as_ref()) {
            let _ = CacheGc::new(&cas, &ac).prune(&GcOptions {
                max_bytes: Some(0),
                max_age: None,
            });
            Ui::stage_item(true, &badge, "Pruned global CAS and Action Cache");
        }
    }

    let ws_msg = format!("workspace: {}", project_dir.display());
    Ui::finish("clean", "ok", &ws_msg);
    0
}

fn is_pid_stale(pid: u32) -> bool {
    #[cfg(unix)]
    unsafe {
        if libc::kill(pid as libc::pid_t, 0) == -1 {
            return std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
        }
    }
    let _ = pid;
    false
}

fn is_stray_tmp_link_file(entry: &std::fs::DirEntry) -> bool {
    let file_name = entry.file_name();
    let Some(name) = file_name.to_str() else {
        return false;
    };
    if !name.starts_with('.') || !name.contains(".tmp_link.") {
        return false;
    }
    name.rsplit('.')
        .nth(1)
        .and_then(|p| p.parse::<u32>().ok())
        .is_some_and(is_pid_stale)
}

fn clean_stray_tmp_links(dir: &std::path::Path) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            if let Ok(ft) = entry.file_type() {
                if ft.is_dir() {
                    clean_stray_tmp_links(&entry.path());
                } else if ft.is_file() && is_stray_tmp_link_file(&entry) {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
    }
}
