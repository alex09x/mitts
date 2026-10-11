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

    println!("Mitts Build Plan Explanation");
    println!("============================");
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
    println!();
    println!(
        "Effective Compilation Units ({} total):",
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
    println!();
    println!("Active Milestone: M3 (Fast parallel fetcher & Cargo.lock resolution)");
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

    let store = match args.cache_dir {
        Some(ref cd) => match SourceStore::new(cd.join("sources")) {
            Ok(s) => s,
            Err(e) => {
                Ui::report_error(&e);
                return 1;
            }
        },
        None => match SourceStore::default_store() {
            Ok(s) => s,
            Err(e) => {
                Ui::report_error(&e);
                return 1;
            }
        },
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
            Ui::stage_item(
                false,
                &Ui::badge_cached(),
                &format!(
                    "{} packages already present in registry",
                    result.already_cached
                ),
            );
            Ui::stage_item(
                true,
                &Ui::badge_download(),
                &format!(
                    "{} packages downloaded & verified (parallel)",
                    result.fetched
                ),
            );
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

pub fn handle_gc(args: &GcArgs) -> i32 {
    let cas = match args.cache_dir {
        Some(ref cd) => match ContentAddressedStore::new(cd.join("cas")) {
            Ok(s) => s,
            Err(e) => {
                Ui::report_error(&e);
                return 1;
            }
        },
        None => match ContentAddressedStore::default_store() {
            Ok(s) => s,
            Err(e) => {
                Ui::report_error(&e);
                return 1;
            }
        },
    };

    let ac = match args.cache_dir {
        Some(ref cd) => match ActionCache::new(cd.join("ac")) {
            Ok(c) => c,
            Err(e) => {
                Ui::report_error(&e);
                return 1;
            }
        },
        None => match ActionCache::default_cache() {
            Ok(c) => c,
            Err(e) => {
                Ui::report_error(&e);
                return 1;
            }
        },
    };

    let gc = CacheGc::new(&cas, &ac);
    let opts = GcOptions {
        max_bytes: args.max_bytes,
        max_age: args.max_age_secs.map(std::time::Duration::from_secs),
    };

    match gc.prune(&opts) {
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
    let mitts_dir = if let Ok(cargo_td) = std::env::var("CARGO_TARGET_DIR") {
        std::path::PathBuf::from(cargo_td).join("mitts")
    } else {
        project_dir.join("target").join("mitts")
    };
    let legacy_mitts_dir = project_dir.join(".mitts");

    Ui::header("clean");

    // 1. Remove fast build lock and build log
    if args.release {
        let lock_file = FastBuildLock::lock_path(project_dir, "release");
        if lock_file.exists() {
            let _ = std::fs::remove_file(&lock_file);
            Ui::stage_item(
                true,
                &Ui::badge_cached(),
                "Removed fast build lock (release)",
            );
        }
    } else {
        let dev_lock = FastBuildLock::lock_path(project_dir, "dev");
        let rel_lock = FastBuildLock::lock_path(project_dir, "release");
        let check_lock = FastBuildLock::lock_path(project_dir, "check");
        let clippy_lock = FastBuildLock::lock_path(project_dir, "clippy");
        let test_lock = FastBuildLock::lock_path(project_dir, "test");
        let legacy_lock = legacy_mitts_dir.join("build.lock");
        for lock_file in [
            &dev_lock,
            &rel_lock,
            &check_lock,
            &clippy_lock,
            &test_lock,
            &legacy_lock,
        ] {
            if lock_file.exists() {
                let _ = std::fs::remove_file(lock_file);
            }
        }
        Ui::stage_item(true, &Ui::badge_cached(), "Removed fast build locks");
    }
    let log_file = mitts_dir.join("build.log");
    if log_file.exists() {
        let _ = std::fs::remove_file(&log_file);
    }
    let legacy_log = legacy_mitts_dir.join("build.log");
    if legacy_log.exists() {
        let _ = std::fs::remove_file(&legacy_log);
    }

    // 2. Remove target directory
    if args.release {
        let rel_dir = mitts_dir.join("release");
        if rel_dir.exists() {
            let _ = std::fs::remove_dir_all(&rel_dir);
            Ui::stage_item(
                true,
                &Ui::badge_cached(),
                "Removed target/mitts/release directory",
            );
        }
    } else {
        if mitts_dir.exists() {
            let _ = std::fs::remove_dir_all(&mitts_dir);
            Ui::stage_item(true, &Ui::badge_cached(), "Removed target/mitts directory");
        }
        if legacy_mitts_dir.exists() {
            let _ = std::fs::remove_dir_all(&legacy_mitts_dir);
        }
    }

    // 3. Clean stray atomic link temporary files in mitts directory (e.g. remaining profile)
    if mitts_dir.exists() {
        clean_stray_tmp_links(&mitts_dir);
    }

    // 4. Clean global cache if requested
    if args.cache {
        let cas = match args.cache_dir {
            Some(ref cd) => ContentAddressedStore::new(cd.join("cas")),
            None => ContentAddressedStore::default_store(),
        };
        let ac = match args.cache_dir {
            Some(ref cd) => ActionCache::new(cd.join("ac")),
            None => ActionCache::default_cache(),
        };
        if let (Ok(cas), Ok(ac)) = (cas, ac) {
            let gc = CacheGc::new(&cas, &ac);
            let _ = gc.prune(&GcOptions {
                max_bytes: Some(0),
                max_age: None,
            });
            Ui::stage_item(
                true,
                &Ui::badge_cached(),
                "Pruned global CAS and Action Cache",
            );
        }
    }

    Ui::finish(
        "clean",
        "ok",
        &format!("workspace: {}", project_dir.display()),
    );
    0
}

fn is_pid_stale(pid: u32) -> bool {
    #[cfg(unix)]
    {
        unsafe {
            if libc::kill(pid as libc::pid_t, 0) == -1 {
                return std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
            }
        }
        false
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

fn is_stray_tmp_link_file(entry: &std::fs::DirEntry) -> bool {
    let file_name = entry.file_name();
    let Some(name) = file_name.to_str() else {
        return false;
    };
    if !name.starts_with('.') || !name.contains(".tmp_link.") {
        return false;
    }
    // Expected format: .{artifact}.tmp_link.{pid}.{counter}
    let parts: Vec<&str> = name.rsplitn(3, '.').collect();
    if parts.len() >= 2 {
        if let Ok(pid) = parts[1].parse::<u32>() {
            return is_pid_stale(pid);
        }
    }
    false
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
