/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use flate2::write::GzEncoder;
use flate2::Compression;
use mitts::adapter::rust::source_lookup::find_crate_source_dir;
use mitts::adapter::rust::RustBuildOptions;
use mitts::adapter::RustAdapter;
use mitts::core::hash::Sha256;
use mitts::core::source_store::{SourceStore, MITTS_OK_MARKER};
use std::fs;
use std::path::Path;
use std::sync::Mutex;
use tempfile::TempDir;

static ENV_MUTEX: Mutex<()> = Mutex::new(());

fn make_crate_archive(name: &str, version: &str, extra_toml: &str) -> (Vec<u8>, String) {
    let mut tar_buf = Vec::new();
    {
        let enc = GzEncoder::new(&mut tar_buf, Compression::default());
        let mut tar = tar::Builder::new(enc);

        let prefix = format!("{}-{}", name, version);
        let toml_bytes = format!(
            "[package]\nname = \"{}\"\nversion = \"{}\"\nedition = \"2021\"\n{}\n",
            name, version, extra_toml
        )
        .into_bytes();
        let mut h1 = tar::Header::new_gnu();
        h1.set_size(toml_bytes.len() as u64);
        h1.set_mode(0o644);
        h1.set_cksum();
        tar.append_data(
            &mut h1,
            format!("{}/Cargo.toml", prefix),
            toml_bytes.as_slice(),
        )
        .unwrap();

        let lib_bytes = b"pub fn valid_fn() -> u32 { 42 }\n";
        let mut h2 = tar::Header::new_gnu();
        h2.set_size(lib_bytes.len() as u64);
        h2.set_mode(0o644);
        h2.set_cksum();
        tar.append_data(&mut h2, format!("{}/src/lib.rs", prefix), &lib_bytes[..])
            .unwrap();

        let enc = tar.into_inner().unwrap();
        enc.finish().unwrap();
    }
    let checksum = Sha256::digest(&tar_buf);
    (tar_buf, checksum)
}

fn with_isolated_env<F: FnOnce(&Path, &Path, &Path)>(f: F) {
    let _guard = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let cargo_home = home.join(".cargo");
    let mitts_cache = tmp.path().join("mitts_cache");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&cargo_home).unwrap();
    fs::create_dir_all(&mitts_cache).unwrap();

    let old_home = std::env::var_os("HOME");
    let old_cargo = std::env::var_os("CARGO_HOME");
    let old_mitts = std::env::var_os("MITTS_CACHE_DIR");

    std::env::set_var("HOME", &home);
    std::env::set_var("CARGO_HOME", &cargo_home);
    std::env::set_var("MITTS_CACHE_DIR", &mitts_cache);

    f(tmp.path(), &cargo_home, &mitts_cache);

    match old_home {
        Some(v) => std::env::set_var("HOME", v),
        None => std::env::remove_var("HOME"),
    }
    match old_cargo {
        Some(v) => std::env::set_var("CARGO_HOME", v),
        None => std::env::remove_var("CARGO_HOME"),
    }
    match old_mitts {
        Some(v) => std::env::set_var("MITTS_CACHE_DIR", v),
        None => std::env::remove_var("MITTS_CACHE_DIR"),
    }
}

#[test]
fn test_t0_b1_custom_and_sparse_registries_rejected_in_lookup_and_explain() {
    with_isolated_env(|root, cargo_home, mitts_cache| {
        // Seed crates.io foo-1.0.0 in Mitts store, Cargo crates.io registry, and git checkouts
        let store = SourceStore::new(mitts_cache.join("sources")).unwrap();
        let (bytes, cksum) = make_crate_archive("foo", "1.0.0", "");
        store
            .unpack_archive("foo", "1.0.0", &bytes, &cksum)
            .unwrap();
        assert!(store.has_source("foo", "1.0.0"));

        let cargo_reg_pkg = cargo_home
            .join("registry")
            .join("src")
            .join("index.crates.io-6f17d22bba15001f")
            .join("foo-1.0.0");
        fs::create_dir_all(cargo_reg_pkg.join("src")).unwrap();
        fs::write(
            cargo_reg_pkg.join("Cargo.toml"),
            "[package]\nname = \"foo\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        fs::write(cargo_reg_pkg.join(".cargo-ok"), "").unwrap();

        let git_checkout = cargo_home
            .join("git")
            .join("checkouts")
            .join("foo-123456")
            .join("abcdef123456");
        fs::create_dir_all(&git_checkout).unwrap();
        fs::write(
            git_checkout.join("Cargo.toml"),
            "[package]\nname = \"foo\"\nversion = \"9.9.9\"\n",
        )
        .unwrap();

        // 1. Direct lookup must reject custom registry+ and sparse+ even though foo-1.0.0 is in crates.io store and git
        assert_eq!(
            find_crate_source_dir(
                "foo",
                "1.0.0",
                Some("registry+https://example.invalid/index")
            ),
            None
        );
        assert_eq!(
            find_crate_source_dir("foo", "1.0.0", Some("sparse+https://example.invalid/")),
            None
        );

        // 2. Explain on project with custom registry+ or sparse+ must fail with explicit error
        for custom_src in [
            "registry+https://example.invalid/index",
            "sparse+https://example.invalid/",
        ] {
            let proj = root.join(if custom_src.starts_with("registry+") {
                "proj_reg"
            } else {
                "proj_sparse"
            });
            fs::create_dir_all(proj.join("src")).unwrap();
            fs::write(
                proj.join("Cargo.toml"),
                "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nfoo = \"1.0.0\"\n",
            )
            .unwrap();
            fs::write(proj.join("src/lib.rs"), "pub fn run() {}\n").unwrap();
            fs::write(
                proj.join("Cargo.lock"),
                format!(
                    "version = 3\n\n[[package]]\nname = \"app\"\nversion = \"0.1.0\"\ndependencies = [\"foo\"]\n\n[[package]]\nname = \"foo\"\nversion = \"1.0.0\"\nsource = \"{}\"\n",
                    custom_src
                ),
            )
            .unwrap();

            let opt = RustBuildOptions {
                manifest_path: Some(proj.join("Cargo.toml")),
                cache_dir: Some(mitts_cache.to_path_buf()),
                ..Default::default()
            };
            let err = RustAdapter::explain_plan(&opt)
                .expect_err("explain must reject non-crates.io registry package");
            let msg = format!("{:#}", err);
            // A git-index registry is refused outright; an unknown sparse registry is
            // contacted and fails on its (unresolvable) host.
            let expected = if custom_src.starts_with("registry+") {
                "sparse registries only"
            } else {
                "example.invalid"
            };
            assert!(
                msg.contains(expected),
                "unexpected error for {}: {}",
                custom_src,
                msg
            );
        }
    });
}

#[test]
fn test_t0_b2_explain_ignores_unmarked_dirs_and_recovers_from_archive() {
    with_isolated_env(|root, cargo_home, mitts_cache| {
        // 1. Seed an incomplete/un-marked directory in Mitts store (no .mitts-ok) with broken source
        let broken_store_dir = mitts_cache.join("sources").join("demo-b2-0.1.0");
        fs::create_dir_all(&broken_store_dir).unwrap();
        fs::write(
            broken_store_dir.join("Cargo.toml"),
            "[package]\nname = \"demo-b2\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        // Notice: no src/lib.rs and no .mitts-ok!

        // 2. Seed an incomplete directory in Cargo registry/src without .cargo-ok
        let cargo_src_dir = cargo_home
            .join("registry")
            .join("src")
            .join("index.crates.io-6f17d22bba15001f")
            .join("demo-b2-0.1.0");
        fs::create_dir_all(&cargo_src_dir).unwrap();
        fs::write(
            cargo_src_dir.join("Cargo.toml"),
            "[package]\nname = \"demo-b2\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        // Notice: no .cargo-ok!

        // 3. Seed valid .crate archive in Cargo registry/cache (read-only source)
        let (archive_bytes, checksum) = make_crate_archive("demo-b2", "0.1.0", "");
        let cargo_cache_index = cargo_home
            .join("registry")
            .join("cache")
            .join("index.crates.io-6f17d22bba15001f");
        fs::create_dir_all(&cargo_cache_index).unwrap();
        fs::write(
            cargo_cache_index.join("demo-b2-0.1.0.crate"),
            &archive_bytes,
        )
        .unwrap();

        // 4. Create a project depending on demo-b2 0.1.0 and run explain
        let proj = root.join("proj_b2");
        fs::create_dir_all(proj.join("src")).unwrap();
        fs::write(
            proj.join("Cargo.toml"),
            "[package]\nname = \"proj-b2\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\ndemo-b2 = \"0.1.0\"\n",
        )
        .unwrap();
        fs::write(proj.join("src/lib.rs"), "pub fn app() {}\n").unwrap();
        fs::write(
            proj.join("Cargo.lock"),
            format!(
                "version = 3\n\n[[package]]\nname = \"demo-b2\"\nversion = \"0.1.0\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\nchecksum = \"{}\"\n\n[[package]]\nname = \"proj-b2\"\nversion = \"0.1.0\"\ndependencies = [\"demo-b2\"]\n",
                checksum
            ),
        )
        .unwrap();

        let opt = RustBuildOptions {
            manifest_path: Some(proj.join("Cargo.toml")),
            cache_dir: Some(mitts_cache.to_path_buf()),
            ..Default::default()
        };
        let plan = RustAdapter::explain_plan(&opt).expect("explain should recover unmarked dir");
        assert!(
            plan.units.iter().any(|u| u.package_name == "demo-b2"),
            "plan must include recovered demo-b2 unit"
        );
        assert!(
            broken_store_dir.join(MITTS_OK_MARKER).is_file()
                && broken_store_dir.join("src/lib.rs").is_file(),
            "unmarked store directory must be replaced with valid .mitts-ok unpacked source"
        );
        assert!(
            !cargo_src_dir.join(".cargo-ok").exists()
                && !cargo_src_dir.join(MITTS_OK_MARKER).exists(),
            "Cargo registry/src must remain untouched"
        );
    });
}

#[test]
fn test_t0_b3_unreachable_git_dependency_fails_explain_with_git_error() {
    with_isolated_env(|root, _cargo_home, mitts_cache| {
        let proj = root.join("proj_b3");
        fs::create_dir_all(proj.join("src")).unwrap();
        fs::write(
            proj.join("Cargo.toml"),
            "[package]\nname = \"proj-b3\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\ngit-dep = { git = \"file:///nonexistent/example/git-dep.git\" }\n",
        )
        .unwrap();
        fs::write(proj.join("src/lib.rs"), "pub fn app() {}\n").unwrap();
        fs::write(
            proj.join("Cargo.lock"),
            "version = 3\n\n[[package]]\nname = \"git-dep\"\nversion = \"0.1.0\"\nsource = \"git+file:///nonexistent/example/git-dep.git#deadbeef12345678\"\n\n[[package]]\nname = \"proj-b3\"\nversion = \"0.1.0\"\ndependencies = [\"git-dep\"]\n",
        )
        .unwrap();

        let opt = RustBuildOptions {
            manifest_path: Some(proj.join("Cargo.toml")),
            cache_dir: Some(mitts_cache.to_path_buf()),
            ..Default::default()
        };
        let err = RustAdapter::explain_plan(&opt)
            .expect_err("explain must fail when the git dependency cannot be cloned");
        let msg = format!("{:#}", err);
        assert!(
            msg.contains("git-dep-0.1.0")
                && msg.contains("git dependency")
                && msg.contains("nonexistent/example/git-dep.git"),
            "unexpected error: {}",
            msg
        );
    });
}

#[test]
fn test_t0_n1_and_n2_dead_pid_staging_cleanup() {
    let tmp = TempDir::new().unwrap();
    let store = SourceStore::new(tmp.path().join("sources")).unwrap();
    let dead_staging = store.locks_dir().join("foo-0.1.0-99999999-0.staging");
    let dead_trash = store.locks_dir().join("foo-0.1.0-99999999-1.trash");
    let fresh_dead = store.locks_dir().join("foo-0.1.0-99999999-2.staging");
    let live_staging = store
        .locks_dir()
        .join(format!("foo-0.1.0-{}-0.staging", std::process::id()));

    for dir in [&dead_staging, &dead_trash, &fresh_dead, &live_staging] {
        fs::create_dir_all(dir).unwrap();
    }
    let stale_time = std::time::SystemTime::now() - std::time::Duration::from_secs(7200);
    for dir in [&dead_staging, &dead_trash, &live_staging] {
        fs::File::open(dir)
            .unwrap()
            .set_modified(stale_time)
            .unwrap();
    }

    assert_eq!(store.cleanup_dead_staging(), 2);
    assert!(!dead_staging.exists() && !dead_trash.exists());
    assert!(fresh_dead.exists() && live_staging.exists());
}

#[test]
fn test_t0_r1_patch_or_path_package_without_source_never_uses_crates_io_store() {
    with_isolated_env(|root, _cargo_home, mitts_cache| {
        // Seed a valid crates.io copy of foo-1.2.3 in Mitts SourceStore with .mitts-ok
        let store_dir = mitts_cache.join("sources/foo-1.2.3");
        fs::create_dir_all(store_dir.join("src")).unwrap();
        fs::write(
            store_dir.join("Cargo.toml"),
            "[package]\nname = \"foo\"\nversion = \"1.2.3\"\nedition = \"2021\"\n",
        )
        .unwrap();
        fs::write(store_dir.join("src/lib.rs"), "pub fn from_crates_io() {}\n").unwrap();
        fs::write(store_dir.join(MITTS_OK_MARKER), "ok\n").unwrap();

        // source == None must never resolve from SourceStore or Cargo registry
        assert_eq!(find_crate_source_dir("foo", "1.2.3", None), None);

        // Workspace patching foo via [patch.crates-io] -> Cargo.lock has foo 1.2.3 without source
        let proj = root.join("proj_r1");
        fs::create_dir_all(proj.join("src")).unwrap();
        fs::write(
            proj.join("Cargo.toml"),
            "[package]\nname = \"proj-r1\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nfoo = \"1.2.3\"\n\n[patch.crates-io]\nfoo = { path = \"vendor/foo\" }\n",
        )
        .unwrap();
        fs::write(proj.join("src/lib.rs"), "pub fn app() {}\n").unwrap();
        fs::write(
            proj.join("Cargo.lock"),
            "version = 3\n\n[[package]]\nname = \"foo\"\nversion = \"1.2.3\"\n\n[[package]]\nname = \"proj-r1\"\nversion = \"0.1.0\"\ndependencies = [\"foo\"]\n",
        )
        .unwrap();

        let opt = RustBuildOptions {
            manifest_path: Some(proj.join("Cargo.toml")),
            cache_dir: Some(mitts_cache.to_path_buf()),
            ..Default::default()
        };
        let err = RustAdapter::explain_plan(&opt).expect_err(
            "explain_plan must reject source == None package outside workspace/path_dependencies instead of substituting crates.io copy",
        );
        let msg = format!("{:#}", err);
        // The patch names a directory that does not exist: the planner reports that
        // directory and never substitutes the crates.io copy.
        assert!(
            msg.contains("vendor/foo")
                || msg.contains("foo-1.2.3 has no source in Cargo.lock (path/patch package)"),
            "unexpected error: {}",
            msg
        );
    });
}

#[test]
fn test_t0_r2_active_cargo_home_honors_explicit_cargo_home_even_when_home_differs() {
    with_isolated_env(|_root, _cargo_home, _mitts_cache| {
        let user = std::env::var("USER").unwrap_or_else(|_| "tester".to_string());
        let explicit_ch = std::path::PathBuf::from(format!("/home/{}/.cargo", user));
        std::env::set_var("HOME", "/tmp/x");
        std::env::set_var("CARGO_HOME", &explicit_ch);
        assert_eq!(SourceStore::active_cargo_home(), Some(explicit_ch));
    });
}
