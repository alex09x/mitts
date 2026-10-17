/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::{BuildMode, RustAdapter, RustBuildOptions};
use mitts::cli::fmt::handle_fmt;
use mitts::cli::handlers::handle_clean;
use mitts::cli::{CleanArgs, FmtArgs};
use mitts::core::action_cache::ActionCache;
use mitts::core::cas::ContentAddressedStore;
use mitts::core::gc::{CacheGc, CacheLease, GcOptions};
use std::collections::HashSet;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use tempfile::TempDir;

fn unique_inode_bytes(dir: &Path, seen: &mut HashSet<(u64, u64)>) -> u64 {
    let mut total = 0;
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                total += unique_inode_bytes(&path, seen);
            } else if let Ok(meta) = entry.metadata() {
                if seen.insert((meta.dev(), meta.ino())) {
                    total += meta.len();
                }
            }
        }
    }
    total
}

fn write_file(base: &Path, rel: &str, content: &str) {
    let p = base.join(rel);
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(p, content).unwrap();
}

#[test]
fn test_b1_gc_prunes_cas_artifacts_and_preserves_during_active_locks() {
    let tmp = TempDir::new().unwrap();
    let proj = tmp.path().join("proj");
    let cache_dir = tmp.path().join("cache");
    write_file(
        &proj,
        "Cargo.toml",
        "[package]\nname = \"b1_pkg\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write_file(
        &proj,
        "src/lib.rs",
        "pub fn compute() -> u64 { 123456789 }\n",
    );

    let opt = RustBuildOptions {
        manifest_path: Some(proj.join("Cargo.toml")),
        cache_dir: Some(cache_dir.clone()),
        ..Default::default()
    };
    let (_, res) = RustAdapter::build(&opt).unwrap();
    assert!(res.success);

    let cas_root = cache_dir.join("cas");
    let ac_root = cache_dir.join("ac");
    let cas = ContentAddressedStore::new(cas_root.clone()).unwrap();
    let ac = ActionCache::new(ac_root.clone()).unwrap();
    let gc = CacheGc::new(&cas, &ac);

    let lease_guard = CacheLease::acquire(Some(&cache_dir));
    let report_lease = gc
        .prune(&GcOptions {
            max_bytes: Some(0),
            max_age: None,
        })
        .unwrap();
    assert!(report_lease.skipped_active_build);
    assert_eq!(report_lease.objects_pruned, 0);
    let mut seen_during_lease = HashSet::new();
    assert!(unique_inode_bytes(&cas_root.join("artifacts"), &mut seen_during_lease) > 0);
    drop(lease_guard);

    let retained_hash = fs::read_dir(cas_root.join("objects"))
        .unwrap()
        .flatten()
        .find_map(|e| e.file_name().into_string().ok())
        .unwrap();
    let fallback_dir = cas_root.join("artifacts");
    let live_fb = fallback_dir.join("liblive_fb-11111111.rlib");
    let live_sc = fallback_dir.join(".liblive_fb-11111111.rlib.cas_hash");
    fs::write(&live_fb, b"live-fallback-bytes").unwrap();
    fs::write(&live_sc, &retained_hash).unwrap();

    let dead_fb = fallback_dir.join("libfallback-deadbeef.rlib");
    let dead_sc = fallback_dir.join(".libfallback-deadbeef.rlib.cas_hash");
    fs::write(&dead_fb, b"fallback-bytes").unwrap();
    fs::write(&dead_sc, "0".repeat(64)).unwrap();

    gc.prune(&GcOptions {
        max_bytes: Some(1_000_000_000),
        max_age: None,
    })
    .unwrap();
    assert!(live_fb.exists() && live_sc.exists());
    assert!(!dead_fb.exists() && !dead_sc.exists());

    let report_idle = gc
        .prune(&GcOptions {
            max_bytes: Some(0),
            max_age: None,
        })
        .unwrap();
    assert!(!report_idle.skipped_active_build);

    let mut seen_after = HashSet::new();
    let bytes_after = unique_inode_bytes(&cas_root.join("objects"), &mut seen_after)
        + unique_inode_bytes(&cas_root.join("artifacts"), &mut seen_after);
    assert_eq!(bytes_after, 0);
    assert!(!live_fb.exists() && !live_sc.exists());
}

#[test]
fn test_b2_clippy_driver_and_user_flags_scoped_to_primary_units() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path().join("ws");
    let ext_dep = tmp.path().join("ext_dep");
    let cache_dir = tmp.path().join("cache");

    write_file(
        &ext_dep,
        "Cargo.toml",
        "[package]\nname = \"ext_dep\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write_file(
        &ext_dep,
        "src/lib.rs",
        "pub fn ext_fn() -> i32 { let unused_ext = 10; 1 }\n",
    );
    write_file(
        &ws,
        "Cargo.toml",
        "[workspace]\nmembers = [\"util\", \"app\"]\nresolver = \"2\"\n",
    );
    write_file(
        &ws,
        "util/Cargo.toml",
        &format!(
            "[package]\nname = \"util\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[dependencies]\next_dep = {{ path = {:?} }}\n",
            ext_dep
        ),
    );
    write_file(&ws, "util/build.rs", "fn main() { let unused_bs = 42; }\n");
    write_file(
        &ws,
        "util/src/lib.rs",
        "pub fn util_fn() -> i32 { ext_dep::ext_fn() }\n",
    );
    write_file(
        &ws,
        "app/Cargo.toml",
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[dependencies]\nutil = { path = \"../util\" }\n",
    );
    write_file(
        &ws,
        "app/src/lib.rs",
        "pub fn app_fn() -> i32 { util::util_fn() }\n",
    );

    let clippy_opt = RustBuildOptions {
        manifest_path: Some(ws.join("Cargo.toml")),
        package: Some("app".to_string()),
        mode: BuildMode::Clippy,
        extra_args: vec!["-D".to_string(), "warnings".to_string()],
        cache_dir: Some(cache_dir.clone()),
        ..Default::default()
    };
    let check_opt = RustBuildOptions {
        manifest_path: Some(ws.join("Cargo.toml")),
        package: Some("app".to_string()),
        mode: BuildMode::Check,
        cache_dir: Some(cache_dir.clone()),
        ..Default::default()
    };

    let exp_clippy = RustAdapter::explain_plan(&clippy_opt).unwrap();
    let exp_check = RustAdapter::explain_plan(&check_opt).unwrap();
    let keys_clippy = RustAdapter::derive_unit_action_keys(&clippy_opt, &exp_clippy).unwrap();
    let keys_check = RustAdapter::derive_unit_action_keys(&check_opt, &exp_check).unwrap();

    for dep_name in ["ext_dep", "util"] {
        let u_clippy = exp_clippy
            .units
            .iter()
            .find(|u| u.package_name == dep_name)
            .unwrap();
        let u_check = exp_check
            .units
            .iter()
            .find(|u| u.package_name == dep_name)
            .unwrap();
        assert_eq!(
            keys_clippy.get(&u_clippy.unit_id()).unwrap(),
            keys_check.get(&u_check.unit_id()).unwrap()
        );
    }

    let (_, res_app) = RustAdapter::build(&clippy_opt).unwrap();
    assert!(res_app.success);

    let clippy_util_opt = RustBuildOptions {
        package: Some("util".to_string()),
        ..clippy_opt
    };
    let (_, res_util) = RustAdapter::build(&clippy_util_opt).unwrap();
    assert!(!res_util.success);
}

#[test]
fn test_b3_nb3_fmt_discovers_only_target_roots_and_ignores_fixture_rs() {
    let tmp = TempDir::new().unwrap();
    let proj = tmp.path().join("fmt_proj");
    write_file(
        &proj,
        "Cargo.toml",
        "[package]\nname = \"fmt_proj\"\nversion = \"0.1.0\"\n\n[[bin]]\nname = \"custom_tool\"\npath = \"tools/custom_tool.rs\"\n",
    );
    write_file(&proj, "src/lib.rs", "pub mod sub;\n");
    write_file(&proj, "src/sub.rs", "pub fn f()->i32{1+2}\n");
    write_file(&proj, "tools/custom_tool.rs", "fn main(){let x=1;}\n");

    let broken_contents = "fn not_valid_rust( {{{{\n";
    write_file(&proj, "tests/ui/broken.rs", broken_contents);

    let fmt_args = FmtArgs {
        manifest_path: Some(proj.join("Cargo.toml")),
        package: None,
        check: false,
        all: false,
        verbose: 0,
        extra_args: vec![],
    };
    assert_eq!(handle_fmt(&fmt_args), 0);

    let formatted_tool = fs::read_to_string(proj.join("tools/custom_tool.rs")).unwrap();
    assert!(formatted_tool.contains("let x = 1;"));
    assert_eq!(
        fs::read_to_string(proj.join("tests/ui/broken.rs")).unwrap(),
        broken_contents
    );
    assert_eq!(
        handle_fmt(&FmtArgs {
            check: true,
            ..fmt_args.clone()
        }),
        0
    );

    let bad_manifest = tmp.path().join("bad_Cargo.toml");
    fs::write(&bad_manifest, "[package\nbroken = ").unwrap();
    assert_eq!(
        handle_fmt(&FmtArgs {
            manifest_path: Some(bad_manifest),
            ..fmt_args
        }),
        1
    );
}

#[test]
fn test_nb1_nb2_check_metadata_only_and_build_upgrade_plus_all_targets() {
    let tmp = TempDir::new().unwrap();
    let proj = tmp.path().join("nb_proj");
    let cache_dir = tmp.path().join("cache");
    write_file(
        &proj,
        "Cargo.toml",
        "[package]\nname = \"nb_proj\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write_file(&proj, "src/lib.rs", "pub fn val() -> i32 { 7 }\n");
    write_file(
        &proj,
        "src/main.rs",
        "fn main() { assert_eq!(nb_proj::val(), 7); }\n",
    );
    write_file(
        &proj,
        "tests/integ.rs",
        "#[test]\nfn t() { assert_eq!(nb_proj::val(), 7); }\n",
    );

    let cli_check = mitts::cli::CheckArgs {
        manifest_path: Some(proj.join("Cargo.toml")),
        all_targets: true,
        cache_dir: Some(cache_dir.clone()),
        ..Default::default()
    };
    assert_eq!(mitts::cli::handle_check(&cli_check), 0);

    let check_opt = RustBuildOptions {
        manifest_path: Some(proj.join("Cargo.toml")),
        mode: BuildMode::Check,
        all_bins: true,
        all_tests: true,
        cache_dir: Some(cache_dir.clone()),
        ..Default::default()
    };
    let (check_plan, check_res) = RustAdapter::build(&check_opt).unwrap();
    assert!(check_res.success);
    assert!(check_plan
        .actions
        .iter()
        .any(|a| a.description.contains("integ")));

    let deps_dir = proj.join("target/mitts/debug/deps");
    assert!(deps_dir.join("libnb_proj.rmeta").exists());
    assert!(!deps_dir.join("libnb_proj.rlib").exists());

    let build_opt = RustBuildOptions {
        manifest_path: Some(proj.join("Cargo.toml")),
        mode: BuildMode::Build,
        cache_dir: Some(cache_dir.clone()),
        ..Default::default()
    };
    let (_, build_res) = RustAdapter::build(&build_opt).unwrap();
    assert!(build_res.success);
    assert!(deps_dir.join("libnb_proj.rlib").exists());

    let meta_artifacts: Vec<String> = fs::read_dir(cache_dir.join("cas/artifacts-meta"))
        .unwrap()
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    assert!(meta_artifacts
        .iter()
        .any(|n| n.starts_with("libnb_proj-") && n.ends_with(".rmeta")));
}

#[test]
fn test_nb4_clean_respects_target_dir_and_refuses_unowned_dirs() {
    let tmp = TempDir::new().unwrap();
    let proj = tmp.path().join("clean_proj");
    let custom_td = tmp.path().join("custom_target");
    let unowned_dir = tmp.path().join("unowned_data");
    write_file(&unowned_dir, "precious.txt", "do not delete");
    write_file(
        &proj,
        "Cargo.toml",
        "[package]\nname = \"clean_proj\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write_file(&proj, "src/lib.rs", "pub fn ok() {}\n");

    let bad_clean = CleanArgs {
        manifest_path: Some(proj.join("Cargo.toml")),
        target_dir: Some(unowned_dir.clone()),
        release: false,
        cache: false,
        cache_dir: None,
    };
    assert_eq!(handle_clean(&bad_clean), 1);
    assert!(unowned_dir.join("precious.txt").exists());

    assert_eq!(
        handle_clean(&CleanArgs {
            target_dir: Some(proj.clone()),
            ..bad_clean.clone()
        }),
        1
    );
    assert!(proj.join("Cargo.toml").exists());

    let opt = RustBuildOptions {
        manifest_path: Some(proj.join("Cargo.toml")),
        target_dir: Some(custom_td.clone()),
        ..Default::default()
    };
    let (_, res) = RustAdapter::build(&opt).unwrap();
    assert!(res.success);
    assert!(custom_td.join(".mitts-target").is_file());
    assert!(!tmp.path().join("CACHEDIR.TAG").exists());

    assert_eq!(
        handle_clean(&CleanArgs {
            target_dir: Some(custom_td.clone()),
            ..bad_clean
        }),
        0
    );
    assert!(!custom_td.exists());
}
