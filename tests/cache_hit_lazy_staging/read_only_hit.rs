/*
 * Mitts Build — build-script cache-hit jobserver and GC regressions
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::*;

#[test]
fn test_multi_target_build_script_dedup_readonly_cas_and_zero_jobserver_tokens_on_hit() {
    let _lock = SERIAL_LOCK.lock().unwrap();
    let cache_dir = TempDir::new().unwrap();
    let ws = TempDir::new().unwrap();

    write_file(
        ws.path().join("Cargo.toml"),
        concat!(
            "[package]\nname = \"multi_target_bs\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n",
            "[[bin]]\nname = \"bin_one\"\npath = \"src/bin/bin_one.rs\"\n\n",
            "[[bin]]\nname = \"bin_two\"\npath = \"src/bin/bin_two.rs\"\n",
        ),
    );
    write_file(
        ws.path().join("build.rs"),
        r##"use std::{env, fs, path::Path};
fn main() {
    let out = env::var("OUT_DIR").unwrap();
    fs::write(Path::new(&out).join("gen.rs"), "pub const MSG: &str = \"dedup_ok\";\n").unwrap();
    println!("cargo:rerun-if-changed=build.rs");
}
"##,
    );
    write_file(
        ws.path().join("src/lib.rs"),
        "include!(concat!(env!(\"OUT_DIR\"), \"/gen.rs\"));\npub fn msg() -> &'static str { MSG }\n",
    );
    write_file(
        ws.path().join("src/bin/bin_one.rs"),
        "fn main() { print!(\"1:{}\", multi_target_bs::msg()); }\n",
    );
    write_file(
        ws.path().join("src/bin/bin_two.rs"),
        "fn main() { print!(\"2:{}\", multi_target_bs::msg()); }\n",
    );

    let opt = RustBuildOptions {
        manifest_path: Some(ws.path().join("Cargo.toml")),
        cache_dir: Some(cache_dir.path().to_path_buf()),
        target_dir: Some(ws.path().join("target")),
        ..Default::default()
    };

    mitts::core::metrics::reset_metrics();
    mitts::core::JobserverPool::reset_tokens_acquired();

    let (_, res1) = RustAdapter::build(&opt).expect("cold build");
    assert!(res1.success, "cold build failed: {}", res1.stderr);
    assert!(!res1.cached);
    let (_, _, cold_bs_hits, cold_bs_misses) = mitts::core::metrics::get_phase_metrics_snapshot();
    assert_eq!(
        cold_bs_misses, 1,
        "build.rs must execute once when lib builds in wave 0"
    );
    assert_eq!(
        cold_bs_hits, 2,
        "dependent bin targets in wave 1 must hit CAS without re-executing build.rs"
    );

    // Remove jobserver dir and make build_scripts CAS entries and .last_used read-only
    let js_dir = cache_dir.path().join("jobserver");
    if js_dir.exists() {
        let _ = fs::remove_dir_all(&js_dir);
    }

    let bs_root = cache_dir.path().join("cas/build_scripts");
    let mut last_used_files = Vec::new();
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    for k in fs::read_dir(&bs_root).unwrap().flatten() {
        for e in fs::read_dir(k.path()).unwrap().flatten() {
            let lu = e.path().join(".last_used");
            if lu.is_file() {
                let mtime = fs::metadata(&lu).unwrap().modified().unwrap();
                #[cfg(unix)]
                {
                    let _ = fs::set_permissions(&lu, fs::Permissions::from_mode(0o444));
                    let _ = fs::set_permissions(e.path(), fs::Permissions::from_mode(0o555));
                }
                last_used_files.push((e.path(), lu, mtime));
            }
        }
    }
    assert!(!last_used_files.is_empty(), "expected .last_used in CAS");

    mitts::core::metrics::reset_metrics();
    mitts::core::JobserverPool::reset_tokens_acquired();

    let (_, res2) = RustAdapter::build(&opt).expect("warm build");

    #[cfg(unix)]
    for (dir, lu, _) in &last_used_files {
        let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o755));
        let _ = fs::set_permissions(lu, fs::Permissions::from_mode(0o644));
    }

    assert!(res2.success, "warm build failed: {}", res2.stderr);
    assert!(res2.cached, "warm build must hit cache 100%");
    assert_eq!(
        mitts::core::JobserverPool::total_tokens_acquired(),
        0,
        "warm cache-hit build must acquire 0 jobserver tokens"
    );
    assert!(
        !js_dir.exists(),
        "warm cache-hit build must not initialize jobserver directory or locks"
    );

    let (_, _, warm_bs_hits, warm_bs_misses) = mitts::core::metrics::get_phase_metrics_snapshot();
    assert_eq!(warm_bs_misses, 0);
    assert_eq!(
        warm_bs_hits, 3,
        "warm build must hit build.rs CAS for all 3 targets with 0 misses"
    );

    for (_, lu, mtime_before) in &last_used_files {
        let mtime_after = fs::metadata(lu).unwrap().modified().unwrap();
        assert_eq!(
            *mtime_before, mtime_after,
            ".last_used must not be mutated on warm cache hit within throttle window"
        );
    }

    // Verify .last_used refreshes once aged past LAST_USED_REFRESH_INTERVAL so GC preserves it
    #[cfg(unix)]
    {
        let (active_entry_dir, active_lu, _) = &last_used_files[0];
        let base_key_dir = active_entry_dir.parent().unwrap();
        let stale_entry_dir = base_key_dir.join("stale_exec_entry_999");
        fs::create_dir_all(&stale_entry_dir).unwrap();
        let stale_lu = stale_entry_dir.join(".last_used");
        fs::write(stale_entry_dir.join("meta.toml"), b"cfgs = []\n").unwrap();
        fs::write(&stale_lu, b"").unwrap();

        let past_sec = (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            - 120) as libc::time_t;
        for p in [active_lu, &stale_lu] {
            let c_path = std::ffi::CString::new(p.to_str().unwrap()).unwrap();
            let tv = [
                libc::timeval {
                    tv_sec: past_sec,
                    tv_usec: 0,
                },
                libc::timeval {
                    tv_sec: past_sec,
                    tv_usec: 0,
                },
            ];
            assert_eq!(unsafe { libc::utimes(c_path.as_ptr(), tv.as_ptr()) }, 0);
        }
        let aged_mtime = fs::metadata(active_lu).unwrap().modified().unwrap();

        let (_, res3) = RustAdapter::build(&opt).expect("warm build after aging .last_used");
        assert!(res3.success && res3.cached);

        let refreshed_mtime = fs::metadata(active_lu).unwrap().modified().unwrap();
        assert!(
            refreshed_mtime > aged_mtime,
            ".last_used must refresh when older than throttle interval"
        );

        let cas = mitts::core::ContentAddressedStore::new(cache_dir.path().join("cas")).unwrap();
        let ac = mitts::core::ActionCache::new(cache_dir.path().join("ac")).unwrap();
        let gc = mitts::core::CacheGc::new(&cas, &ac);
        gc.prune(&mitts::core::GcOptions {
            max_bytes: None,
            max_age: Some(std::time::Duration::from_secs(90)),
        })
        .unwrap();

        assert!(
            active_entry_dir.exists(),
            "active build.rs entry must be preserved by GC after throttled .last_used refresh"
        );
        assert!(
            !stale_entry_dir.exists(),
            "stale build.rs entry must be pruned by GC"
        );
    }
}

#[test]
fn test_build_dep_artifact_in_place_mutation_invalidates_cas_restore() {
    use mitts::adapter::rust::build_script::try_restore_build_script;

    let _lock = SERIAL_LOCK.lock().unwrap();
    let cache_dir = TempDir::new().unwrap();
    let ws = TempDir::new().unwrap();

    write_file(
        ws.path().join("Cargo.toml"),
        "[package]\nname = \"dep_inval_pkg\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write_file(
        ws.path().join("build.rs"),
        r##"use std::{env, fs, path::Path};
fn main() {
    let out = env::var("OUT_DIR").unwrap();
    fs::write(Path::new(&out).join("info.rs"), "pub const OK: bool = true;\n").unwrap();
    println!("cargo:rerun-if-changed=build.rs");
}
"##,
    );
    write_file(
        ws.path().join("src/lib.rs"),
        "pub fn ok() -> bool { true }\n",
    );

    let opt = RustBuildOptions {
        manifest_path: Some(ws.path().join("Cargo.toml")),
        cache_dir: Some(cache_dir.path().to_path_buf()),
        target_dir: Some(ws.path().join("target")),
        ..Default::default()
    };
    let expl = RustAdapter::explain_plan(&opt).unwrap();
    let unit = expl
        .units
        .iter()
        .find(|u| u.package_name == "dep_inval_pkg")
        .unwrap();

    // Create a synthetic build-dep artifact file and a staging dir
    let dep_artifact = ws.path().join("libfake_build_dep.rlib");
    fs::write(&dep_artifact, b"rlib_payload_v1").unwrap();
    let externs = vec![("fake_build_dep".to_string(), dep_artifact.clone())];
    let artifacts_dir = cache_dir.path().join("cas/artifacts");
    fs::create_dir_all(&artifacts_dir).unwrap();

    let miss_before =
        try_restore_build_script(unit, false, &[], Some(&artifacts_dir), &externs, None).unwrap();
    assert!(miss_before.is_none());

    let stage1 = TempDir::new().unwrap();
    let _d1 = execute_build_script(
        unit,
        stage1.path(),
        false,
        &[],
        Some(&artifacts_dir),
        &externs,
        None,
    )
    .unwrap();

    // Unchanged build-dep artifact at the same path hits CAS
    let cas_hit =
        try_restore_build_script(unit, false, &[], Some(&artifacts_dir), &externs, None).unwrap();
    assert!(
        cas_hit.is_some(),
        "unchanged build-dep artifact must hit CAS"
    );

    // Mutate build-dep artifact in place at the exact same PathBuf (even same byte length)
    fs::write(&dep_artifact, b"rlib_payload_v2").unwrap();

    let cas_after_mutation =
        try_restore_build_script(unit, false, &[], Some(&artifacts_dir), &externs, None).unwrap();
    assert!(
        cas_after_mutation.is_none(),
        "mutating build-dep artifact at same path must invalidate CAS restore"
    );
}
