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
use mitts::core::gc::{CacheGc, GcOptions};
use mitts::core::lock::FileLock;
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

#[test]
fn test_b1_gc_prunes_cas_artifacts_and_preserves_during_active_locks() {
    let tmp = TempDir::new().unwrap();
    let proj = tmp.path().join("proj");
    let cache_dir = tmp.path().join("cache");
    fs::create_dir_all(proj.join("src")).unwrap();
    fs::write(
        proj.join("Cargo.toml"),
        "[package]\nname = \"b1_pkg\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(
        proj.join("src/lib.rs"),
        "pub fn compute() -> u64 { 123456789 }\n",
    )
    .unwrap();

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

    let mut seen_before = HashSet::new();
    assert!(unique_inode_bytes(&cas_root, &mut seen_before) > 0);
    assert!(cas_root.join("artifacts").exists());

    // Hold an active lock in ac.locks_dir() to simulate an in-flight build that may still link
    // against cas/artifacts of an already-completed upstream dependency.
    let active_lock_path = ac.lock_path("active_unit_key");
    let active_guard = FileLock::acquire_exclusive(&active_lock_path).unwrap();

    let gc = CacheGc::new(&cas, &ac);
    let report_active = gc
        .prune(&GcOptions {
            max_bytes: Some(0),
            max_age: None,
        })
        .unwrap();
    let mut seen_during_active = HashSet::new();
    let artifacts_bytes_during =
        unique_inode_bytes(&cas_root.join("artifacts"), &mut seen_during_active);
    assert!(artifacts_bytes_during > 0);
    assert!(report_active.bytes_retained >= artifacts_bytes_during);

    drop(active_guard);
    let _ = fs::remove_file(&active_lock_path);

    // Also create a synthetic fallback copy in cas/artifacts with a .cas_hash sidecar pointing
    // to a missing/pruned object hash to verify non-hardlink fallback cleanup.
    let fallback_dir = cas_root.join("artifacts");
    fs::create_dir_all(&fallback_dir).unwrap();
    let fallback_file = fallback_dir.join("libfallback-deadbeef.rlib");
    let fallback_sidecar = fallback_dir.join(".libfallback-deadbeef.rlib.cas_hash");
    fs::write(&fallback_file, b"fallback-bytes").unwrap();
    fs::write(
        &fallback_sidecar,
        "0000000000000000000000000000000000000000000000000000000000000000",
    )
    .unwrap();

    // Now prune with no active locks and max_bytes = Some(0).
    let _report_idle = gc
        .prune(&GcOptions {
            max_bytes: Some(0),
            max_age: None,
        })
        .unwrap();

    let mut seen_after = HashSet::new();
    let bytes_after = unique_inode_bytes(&cas_root.join("objects"), &mut seen_after)
        + unique_inode_bytes(&cas_root.join("artifacts"), &mut seen_after);
    assert_eq!(
        bytes_after, 0,
        "expected 0 unique inode bytes in cas/objects + cas/artifacts after idle prune(max_bytes=0), got {}",
        bytes_after
    );
    assert!(!fallback_file.exists());
    assert!(!fallback_sidecar.exists());
}

#[test]
fn test_b2_clippy_driver_and_user_flags_scoped_to_primary_units() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path().join("ws");
    let ext_dep = tmp.path().join("ext_dep");
    let cache_dir = tmp.path().join("cache");

    // External path dependency with an unused variable warning.
    fs::create_dir_all(ext_dep.join("src")).unwrap();
    fs::write(
        ext_dep.join("Cargo.toml"),
        "[package]\nname = \"ext_dep\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(
        ext_dep.join("src/lib.rs"),
        "pub fn ext_fn() -> i32 { let unused_ext = 10; 1 }\n",
    )
    .unwrap();

    // Workspace with `util` (also having an unused variable warning) and `app` (clean).
    fs::create_dir_all(ws.join("util/src")).unwrap();
    fs::create_dir_all(ws.join("app/src")).unwrap();
    fs::write(
        ws.join("Cargo.toml"),
        "[workspace]\nmembers = [\"util\", \"app\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    fs::write(
        ws.join("util/Cargo.toml"),
        format!(
            "[package]\nname = \"util\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[dependencies]\next_dep = {{ path = {:?} }}\n",
            ext_dep
        ),
    )
    .unwrap();
    fs::write(
        ws.join("util/src/lib.rs"),
        "pub fn util_fn() -> i32 { let unused_util = 20; ext_dep::ext_fn() }\n",
    )
    .unwrap();
    fs::write(
        ws.join("app/Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[dependencies]\nutil = { path = \"../util\" }\n",
    )
    .unwrap();
    fs::write(
        ws.join("app/src/lib.rs"),
        "pub fn app_fn() -> i32 { util::util_fn() }\n",
    )
    .unwrap();

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
        let k_clippy = keys_clippy.get(&u_clippy.unit_id()).unwrap();
        let k_check = keys_check.get(&u_check.unit_id()).unwrap();
        assert_eq!(
            k_clippy, k_check,
            "non-primary dependency `{dep_name}` must share the same action key between `mitts clippy -p app -- -D warnings` and `mitts check -p app`"
        );
    }

    // Running `mitts clippy -p app -- -D warnings` must succeed even though `util` and `ext_dep` have unused variables.
    let (_, res) = RustAdapter::build(&clippy_opt).unwrap();
    assert!(
        res.success,
        "expected clippy -p app -- -D warnings to succeed when non-primary deps have warnings"
    );
}

#[test]
fn test_b3_nb3_fmt_discovers_only_target_roots_and_ignores_fixture_rs() {
    let tmp = TempDir::new().unwrap();
    let proj = tmp.path().join("fmt_proj");
    fs::create_dir_all(proj.join("src")).unwrap();
    fs::create_dir_all(proj.join("tools")).unwrap();
    fs::create_dir_all(proj.join("tests/ui")).unwrap();

    fs::write(
        proj.join("Cargo.toml"),
        "[package]\nname = \"fmt_proj\"\nversion = \"0.1.0\"\n\n[[bin]]\nname = \"custom_tool\"\npath = \"tools/custom_tool.rs\"\n",
    )
    .unwrap();
    fs::write(proj.join("src/lib.rs"), "pub mod sub;\n").unwrap();
    fs::write(proj.join("src/sub.rs"), "pub fn f()->i32{1+2}\n").unwrap();
    fs::write(proj.join("tools/custom_tool.rs"), "fn main(){let x=1;}\n").unwrap();

    // Unreferenced UI test fixture with invalid syntax that Cargo/rustfmt must NOT touch.
    let broken_fixture = proj.join("tests/ui/broken.rs");
    let broken_contents = "fn not_valid_rust( {{{{\n";
    fs::write(&broken_fixture, broken_contents).unwrap();

    let fmt_args = FmtArgs {
        manifest_path: Some(proj.join("Cargo.toml")),
        package: None,
        check: false,
        all: false,
        verbose: 0,
        extra_args: vec![],
    };
    assert_eq!(handle_fmt(&fmt_args), 0);

    // Verify `tools/custom_tool.rs` and `src/sub.rs` were formatted, while `tests/ui/broken.rs` was untouched.
    let formatted_tool = fs::read_to_string(proj.join("tools/custom_tool.rs")).unwrap();
    assert!(
        formatted_tool.contains("let x = 1;"),
        "expected custom [[bin]] target to be formatted, got: {formatted_tool}"
    );
    assert_eq!(
        fs::read_to_string(&broken_fixture).unwrap(),
        broken_contents
    );

    let check_args = FmtArgs {
        check: true,
        ..fmt_args
    };
    assert_eq!(handle_fmt(&check_args), 0);
}

#[test]
fn test_nb1_nb2_check_metadata_only_and_build_upgrade_plus_all_targets() {
    let tmp = TempDir::new().unwrap();
    let proj = tmp.path().join("nb_proj");
    let cache_dir = tmp.path().join("cache");
    fs::create_dir_all(proj.join("src")).unwrap();
    fs::create_dir_all(proj.join("tests")).unwrap();

    fs::write(
        proj.join("Cargo.toml"),
        "[package]\nname = \"nb_proj\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(proj.join("src/lib.rs"), "pub fn val() -> i32 { 7 }\n").unwrap();
    fs::write(
        proj.join("src/main.rs"),
        "fn main() { assert_eq!(nb_proj::val(), 7); }\n",
    )
    .unwrap();
    fs::write(
        proj.join("tests/integ.rs"),
        "#[test]\nfn t() { assert_eq!(nb_proj::val(), 7); }\n",
    )
    .unwrap();

    // 1. Check with all_targets = true via CLI CheckArgs and via RustAdapter::build.
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
    assert!(
        check_plan
            .actions
            .iter()
            .any(|a| a.description.contains("integ")),
        "expected --all-targets check to include integration test target"
    );

    let deps_dir = proj.join("target/mitts/debug/deps");
    assert!(deps_dir.join("libnb_proj.rmeta").exists());
    assert!(
        !deps_dir.join("libnb_proj.rlib").exists(),
        "expected metadata-only emission (.rmeta without .rlib) during `mitts check`"
    );

    // 2. Subsequent `mitts build` with the same cache_dir must upgrade the library to a full `.rlib` and link the binary.
    let build_opt = RustBuildOptions {
        manifest_path: Some(proj.join("Cargo.toml")),
        mode: BuildMode::Build,
        cache_dir: Some(cache_dir),
        ..Default::default()
    };
    let (_, build_res) = RustAdapter::build(&build_opt).unwrap();
    assert!(
        build_res.success,
        "expected `mitts build` after `mitts check` to produce `.rlib` and link binary cleanly"
    );
    assert!(deps_dir.join("libnb_proj.rlib").exists());
}

#[test]
fn test_nb4_clean_respects_target_dir() {
    let tmp = TempDir::new().unwrap();
    let proj = tmp.path().join("clean_proj");
    let custom_td = tmp.path().join("custom_target");
    fs::create_dir_all(proj.join("src")).unwrap();
    fs::write(
        proj.join("Cargo.toml"),
        "[package]\nname = \"clean_proj\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(proj.join("src/lib.rs"), "pub fn ok() {}\n").unwrap();

    let opt = RustBuildOptions {
        manifest_path: Some(proj.join("Cargo.toml")),
        target_dir: Some(custom_td.clone()),
        ..Default::default()
    };
    let (_, res) = RustAdapter::build(&opt).unwrap();
    assert!(res.success);
    assert!(custom_td.exists());

    let clean_args = CleanArgs {
        manifest_path: Some(proj.join("Cargo.toml")),
        target_dir: Some(custom_td.clone()),
        release: false,
        cache: false,
        cache_dir: None,
    };
    assert_eq!(handle_clean(&clean_args), 0);
    assert!(
        !custom_td.exists(),
        "expected custom target_dir to be removed by mitts clean --target-dir"
    );
}
