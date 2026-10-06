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
use crate::cli::{ExplainArgs, FetchArgs, GcArgs, Ui};
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
        println!("     Source:      {}", unit.src_path.display());
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
