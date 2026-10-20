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
use mitts::cli::{
    fmt::handle_fmt,
    handlers::handle_clean,
    test::{handle_test, TestArgs},
    CleanArgs, FmtArgs,
};
use mitts::core::{
    action_cache::ActionCache,
    cas::ContentAddressedStore,
    gc::{CacheGc, CacheLease, GcOptions},
};
use std::{collections::HashSet, fs, os::unix::fs::MetadataExt, path::Path};
use tempfile::TempDir;

fn unique_inode_bytes(dir: &Path, seen: &mut HashSet<(u64, u64)>) -> u64 {
    fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| {
            let p = e.path();
            if p.is_dir() {
                unique_inode_bytes(&p, seen)
            } else {
                e.metadata()
                    .ok()
                    .filter(|m| seen.insert((m.dev(), m.ino())))
                    .map_or(0, |m| m.len())
            }
        })
        .sum()
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
    let dt_ready = proj.join("dt_ready");
    let dt_done = proj.join("dt_done");
    write_file(
        &proj,
        "Cargo.toml",
        "[package]\nname = \"b1_pkg\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write_file(
        &proj,
        "src/lib.rs",
        &format!(
            "/// ```\n/// std::fs::write({dt_ready:?}, \"1\").unwrap();\n/// assert!((0..24_000).any(|_| std::path::Path::new({dt_done:?}).exists() || {{ std::thread::sleep(std::time::Duration::from_millis(5)); false }}));\n/// assert_eq!(b1_pkg::compute(), 123456789);\n/// ```\npub fn compute() -> u64 {{ 123456789 }}\n"
        ),
    );

    let cas_root = cache_dir.join("cas");
    let ac_root = cache_dir.join("ac");
    let cas_bg = ContentAddressedStore::new(cas_root.clone()).unwrap();
    let ac_bg = ActionCache::new(ac_root.clone()).unwrap();
    let zero_gc = GcOptions {
        max_bytes: Some(0),
        max_age: None,
    };
    let zero_gc_bg = zero_gc.clone();
    let waiter = std::thread::spawn(move || {
        assert!((0..24_000).any(|_| dt_ready.exists() || {
            std::thread::sleep(std::time::Duration::from_millis(5));
            false
        }));
        let rep = CacheGc::new(&cas_bg, &ac_bg).prune(&zero_gc_bg).unwrap();
        fs::write(&dt_done, "1").unwrap();
        rep
    });
    assert_eq!(
        handle_test(&TestArgs {
            manifest_path: Some(proj.join("Cargo.toml")),
            cache_dir: Some(cache_dir.clone()),
            doc: true,
            ..Default::default()
        }),
        0
    );
    let report_doctest = waiter.join().unwrap();
    assert!(report_doctest.skipped_active_build && report_doctest.objects_pruned == 0);
    let cas = ContentAddressedStore::new(cas_root.clone()).unwrap();
    let ac = ActionCache::new(ac_root).unwrap();
    let gc = CacheGc::new(&cas, &ac);
    let lease_guard = CacheLease::acquire(Some(&cache_dir));
    let report_lease = gc.prune(&zero_gc).unwrap();
    assert!(report_lease.skipped_active_build && report_lease.objects_pruned == 0);
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
    assert!(live_fb.exists() && live_sc.exists() && !dead_fb.exists() && !dead_sc.exists());
    assert!(!gc.prune(&zero_gc).unwrap().skipped_active_build);
    let mut seen_after = HashSet::new();
    let bytes_after = unique_inode_bytes(&cas_root.join("objects"), &mut seen_after)
        + unique_inode_bytes(&cas_root.join("artifacts"), &mut seen_after);
    assert_eq!(bytes_after, 0);
    assert!(!live_fb.exists() && !live_sc.exists());
}

#[test]
fn test_b2_clippy_driver_and_user_flags_scoped_to_primary_units() {
    let isolated_cache = tempfile::tempdir().expect("isolated cache dir");
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path().join("ws");
    let ext_dep = tmp.path().join("ext_dep");
    let cache_dir = tmp.path().join("cache");

    let util_toml = format!(
        "[package]\nname = \"util\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[dependencies]\next_dep = {{ path = {ext_dep:?} }}\n"
    );
    for (base, rel, text) in [
        (
            &ext_dep,
            "Cargo.toml",
            "[package]\nname=\"ext_dep\"\nversion=\"0.1.0\"\n",
        ),
        (
            &ext_dep,
            "src/lib.rs",
            "pub fn ext_fn() -> i32 { let unused_ext = 10; 1 }\n",
        ),
        (
            &ws,
            "Cargo.toml",
            "[workspace]\nmembers=[\"util\",\"app\"]\nresolver=\"2\"\n",
        ),
        (&ws, "util/Cargo.toml", util_toml.as_str()),
        (&ws, "util/build.rs", "fn main() { let unused_bs = 42; }\n"),
        (
            &ws,
            "util/src/lib.rs",
            "pub fn util_fn() -> i32 { ext_dep::ext_fn() }\n",
        ),
        (
            &ws,
            "app/Cargo.toml",
            "[package]\nname=\"app\"\nversion=\"0.1.0\"\n[dependencies]\nutil={path=\"../util\"}\n",
        ),
        (
            &ws,
            "app/src/lib.rs",
            "pub fn app_fn() -> i32 { util::util_fn() }\n",
        ),
    ] {
        write_file(base, rel, text);
    }

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
        cache_dir: Some(cache_dir),
        ..Default::default()
    };

    let exp_clippy = RustAdapter::explain_plan(&clippy_opt).unwrap();
    let exp_check = RustAdapter::explain_plan(&check_opt).unwrap();
    let keys_clippy = RustAdapter::derive_unit_action_keys(&clippy_opt, &exp_clippy).unwrap();
    let keys_check = RustAdapter::derive_unit_action_keys(&check_opt, &exp_check).unwrap();

    for dep_name in ["ext_dep", "util"] {
        let u_c = exp_clippy
            .units
            .iter()
            .find(|u| u.package_name == dep_name)
            .unwrap();
        let u_k = exp_check
            .units
            .iter()
            .find(|u| u.package_name == dep_name)
            .unwrap();
        assert_eq!(
            keys_clippy.get(&u_c.unit_id()),
            keys_check.get(&u_k.unit_id())
        );
    }

    assert!(RustAdapter::build(&clippy_opt).unwrap().1.success);
    assert!(
        !RustAdapter::build(&RustBuildOptions {
            cache_dir: Some(isolated_cache.path().to_path_buf()),
            package: Some("util".to_string()),
            ..clippy_opt
        })
        .unwrap()
        .1
        .success
    );
}

#[test]
fn test_b3_nb3_fmt_discovers_only_target_roots_and_ignores_fixture_rs() {
    let tmp = TempDir::new().unwrap();
    let proj = tmp.path().join("fmt_proj");
    write_file(
        &proj,
        "Cargo.toml",
        "[package]\nname = \"fmt_proj\"\nversion = \"0.1.0\"\n[[bin]]\nname = \"custom_tool\"\npath = \"tools/custom_tool.rs\"\n",
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
    assert!(fs::read_to_string(proj.join("tools/custom_tool.rs"))
        .unwrap()
        .contains("let x = 1;"));
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
    write_file(&proj, "src/main.rs", "fn main() { nb_proj::val(); }\n");
    write_file(&proj, "tests/integ.rs", "#[test] fn t() {}\n");

    assert_eq!(
        mitts::cli::handle_check(&mitts::cli::CheckArgs {
            manifest_path: Some(proj.join("Cargo.toml")),
            all_targets: true,
            cache_dir: Some(cache_dir.clone()),
            ..Default::default()
        }),
        0
    );

    let (check_plan, check_res) = RustAdapter::build(&RustBuildOptions {
        manifest_path: Some(proj.join("Cargo.toml")),
        mode: BuildMode::Check,
        all_bins: true,
        all_tests: true,
        cache_dir: Some(cache_dir.clone()),
        ..Default::default()
    })
    .unwrap();
    assert!(
        check_res.success
            && check_plan
                .actions
                .iter()
                .any(|a| a.description.contains("integ"))
    );

    let deps_dir = proj.join("target/mitts/debug/deps");
    assert!(
        deps_dir.join("libnb_proj.rmeta").exists() && !deps_dir.join("libnb_proj.rlib").exists()
    );

    let (_, build_res) = RustAdapter::build(&RustBuildOptions {
        manifest_path: Some(proj.join("Cargo.toml")),
        mode: BuildMode::Build,
        cache_dir: Some(cache_dir.clone()),
        ..Default::default()
    })
    .unwrap();
    assert!(build_res.success && deps_dir.join("libnb_proj.rlib").exists());
    assert!(fs::read_dir(cache_dir.join("cas/artifacts-meta"))
        .unwrap()
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .any(|n| n.starts_with("libnb_proj-") && n.ends_with(".rmeta")));
}

#[test]
fn test_nb4_clean_respects_target_dir_and_refuses_unowned_dirs() {
    let isolated_cache = tempfile::tempdir().expect("isolated cache dir");
    let tmp = TempDir::new().unwrap();
    let proj = tmp.path().join("clean_proj");
    let custom_td = tmp.path().join("custom_target");
    let unowned_dir = tmp.path().join("unowned_data");
    write_file(&unowned_dir, "precious.txt", "do not delete");
    write_file(
        &proj,
        "Cargo.toml",
        "[package]\nname=\"cp\"\nversion=\"0.1.0\"\n",
    );
    write_file(&proj, "src/lib.rs", "pub fn ok() {}\n");

    let bad_clean = CleanArgs {
        manifest_path: Some(proj.join("Cargo.toml")),
        target_dir: Some(unowned_dir.clone()),
        release: false,
        cache: false,
        cache_dir: Some(isolated_cache.path().to_path_buf()),
    };
    assert_eq!(handle_clean(&bad_clean), 1);
    assert!(unowned_dir.join("precious.txt").exists());
    assert_eq!(
        handle_clean(&CleanArgs {
            cache_dir: Some(isolated_cache.path().to_path_buf()),
            target_dir: Some(proj.clone()),
            ..bad_clean.clone()
        }),
        1
    );
    assert!(proj.join("Cargo.toml").exists());

    let (_, res) = RustAdapter::build(&RustBuildOptions {
        cache_dir: Some(isolated_cache.path().to_path_buf()),
        manifest_path: Some(proj.join("Cargo.toml")),
        target_dir: Some(custom_td.clone()),
        ..Default::default()
    })
    .unwrap();
    assert!(
        res.success
            && custom_td.join(".mitts-target").is_file()
            && !tmp.path().join("CACHEDIR.TAG").exists()
    );
    assert_eq!(
        handle_clean(&CleanArgs {
            cache_dir: Some(isolated_cache.path().to_path_buf()),
            target_dir: Some(custom_td.clone()),
            ..bad_clean
        }),
        0
    );
    assert!(!custom_td.exists());
}
