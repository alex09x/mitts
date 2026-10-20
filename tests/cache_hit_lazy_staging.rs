/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::rust::build_script::execute_build_script;
use mitts::adapter::{RustAdapter, RustBuildOptions};
use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

static SERIAL_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct EnvGuard {
    key: &'static str,
    original: Option<String>,
}

impl EnvGuard {
    fn set(key: &'static str, val: &str) -> Self {
        let original = std::env::var(key).ok();
        std::env::set_var(key, val);
        Self { key, original }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match &self.original {
            Some(v) => std::env::set_var(self.key, v),
            None => std::env::remove_var(self.key),
        }
    }
}

fn write_file(path: impl AsRef<Path>, content: &str) {
    let p = path.as_ref();
    if let Some(parent) = p.parent() {
        let _ = fs::create_dir_all(parent);
    }
    fs::write(p, content).unwrap();
}

#[test]
fn test_cold_miss_then_full_cache_hit_and_no_staging_paths() {
    let _lock = SERIAL_LOCK.lock().unwrap();
    let cache_dir = TempDir::new().unwrap();
    let ws = TempDir::new().unwrap();

    write_file(
        ws.path().join("Cargo.toml"),
        "[workspace]\nmembers = [\"codegen_lib\", \"codegen_bin\"]\nresolver = \"2\"\n",
    );
    write_file(
        ws.path().join("codegen_lib/Cargo.toml"),
        "[package]\nname = \"codegen_lib\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write_file(
        ws.path().join("codegen_lib/build.rs"),
        r##"use std::{env, fs, path::Path};
fn main() {
    let out = env::var("OUT_DIR").unwrap();
    fs::write(Path::new(&out).join("hello.rs"), "pub fn message() -> &'static str { \"lazy staging works!\" }\n").unwrap();
    println!("cargo:rerun-if-changed=build.rs");
}
"##,
    );
    write_file(
        ws.path().join("codegen_lib/src/lib.rs"),
        "include!(concat!(env!(\"OUT_DIR\"), \"/hello.rs\"));\npub fn get_message() -> &'static str { message() }\n",
    );
    write_file(
        ws.path().join("codegen_bin/Cargo.toml"),
        "[package]\nname = \"codegen_bin\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\ncodegen_lib = { path = \"../codegen_lib\" }\n",
    );
    write_file(
        ws.path().join("codegen_bin/src/main.rs"),
        "fn main() { print!(\"{}\", codegen_lib::get_message()); }\n",
    );

    let opt = RustBuildOptions {
        manifest_path: Some(ws.path().join("Cargo.toml")),
        cache_dir: Some(cache_dir.path().to_path_buf()),
        target_dir: Some(ws.path().join("target")),
        ..Default::default()
    };

    // 1. Cold build compiles both and populates CAS
    let (_, res1) = RustAdapter::build(&opt).expect("cold build");
    assert!(res1.success, "cold build failed: {}", res1.stderr);
    assert!(!res1.cached, "cold build must miss cache");

    let bin_path = ws.path().join("target/debug/codegen_bin");
    assert!(bin_path.is_file(), "binary must exist after cold build");
    let out1 = Command::new(&bin_path).output().expect("run codegen_bin 1");
    assert!(out1.status.success());
    assert_eq!(String::from_utf8_lossy(&out1.stdout), "lazy staging works!");

    // 2. Observable mechanism: Make CAS tmp directory read-only (0o555).
    // If warm build attempts to create a staging directory inside cas/tmp, it will fail.
    // Lazy staging avoids creating any staging path on cache hit, so warm build succeeds.
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    let cas_tmp = cache_dir.path().join("cas/tmp");
    #[cfg(unix)]
    if cas_tmp.is_dir() {
        let _ = fs::set_permissions(&cas_tmp, fs::Permissions::from_mode(0o555));
    }

    let (_, res2) = RustAdapter::build(&opt).expect("warm build");

    #[cfg(unix)]
    if cas_tmp.is_dir() {
        let _ = fs::set_permissions(&cas_tmp, fs::Permissions::from_mode(0o755));
    }

    assert!(res2.success, "warm build failed: {}", res2.stderr);
    assert!(res2.cached, "warm build must hit cache 100%");

    let out2 = Command::new(&bin_path).output().expect("run codegen_bin 2");
    assert!(out2.status.success());
    assert_eq!(String::from_utf8_lossy(&out2.stdout), "lazy staging works!");

    if cas_tmp.is_dir() {
        let entries: Vec<_> = fs::read_dir(&cas_tmp)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("stage_"))
            .collect();
        assert!(
            entries.is_empty(),
            "cas/tmp must contain no stage dirs, found: {:?}",
            entries
        );
    }

    // 3. Direct check: execute_build_script with an empty staging path must NOT create any
    // directory tree on a cache hit.
    let expl = RustAdapter::explain_plan(&opt).expect("explain plan");
    let lib_unit = expl
        .units
        .iter()
        .find(|u| u.package_name == "codegen_lib")
        .unwrap();

    let empty_staging = TempDir::new().unwrap();
    let artifacts_dir = cache_dir.path().join("cas/artifacts");
    let d_hit = execute_build_script(
        lib_unit,
        empty_staging.path(),
        false,
        &[],
        Some(&artifacts_dir),
        &[],
        None,
    )
    .unwrap();

    assert!(d_hit.out_dir.as_ref().unwrap().join("hello.rs").is_file());
    let staging_contents: Vec<_> = fs::read_dir(empty_staging.path())
        .unwrap()
        .flatten()
        .collect();
    assert!(
        staging_contents.is_empty(),
        "Cache hit must not create any staging dir tree, found: {:?}",
        staging_contents
    );
}

#[test]
fn test_tracked_file_and_env_change_invalidates_build_script_and_dependent_unit() {
    let _lock = SERIAL_LOCK.lock().unwrap();
    let cache_dir = TempDir::new().unwrap();
    let ws = TempDir::new().unwrap();

    write_file(
        ws.path().join("Cargo.toml"),
        "[package]\nname = \"inval_crate\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write_file(
        ws.path().join("build.rs"),
        r##"use std::{env, fs, path::Path};
fn main() {
    println!("cargo:rerun-if-changed=tracked.txt");
    println!("cargo:rerun-if-env-changed=TEST_LAZY_ENV_VAR");
    let out_dir = env::var("OUT_DIR").unwrap();
    let f_val = fs::read_to_string("tracked.txt").unwrap_or_else(|_| "no_file".to_string());
    let e_val = env::var("TEST_LAZY_ENV_VAR").unwrap_or_else(|_| "no_env".to_string());
    fs::write(Path::new(&out_dir).join("generated.rs"), format!("pub const VAL: &str = \"file={},env={}\";\n", f_val.trim(), e_val.trim())).unwrap();
}
"##,
    );
    write_file(ws.path().join("src/main.rs"), "include!(concat!(env!(\"OUT_DIR\"), \"/generated.rs\"));\nfn main() { print!(\"{}\", VAL); }\n");
    write_file(ws.path().join("tracked.txt"), "v1");
    let _env = EnvGuard::set("TEST_LAZY_ENV_VAR", "e1");

    let opt = RustBuildOptions {
        manifest_path: Some(ws.path().join("Cargo.toml")),
        cache_dir: Some(cache_dir.path().to_path_buf()),
        target_dir: Some(ws.path().join("target")),
        ..Default::default()
    };

    // Cold build
    let (_, res1) = RustAdapter::build(&opt).expect("cold build");
    assert!(res1.success);
    assert!(!res1.cached);
    let bin = ws.path().join("target/debug/inval_crate");
    let out1 = Command::new(&bin).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out1.stdout), "file=v1,env=e1");

    // Warm hit
    let (_, res2) = RustAdapter::build(&opt).expect("warm build 1");
    assert!(res2.success);
    assert!(res2.cached, "must hit cache when inputs unchanged");

    // Tracked file change: invalidates build script and dependent unit
    write_file(ws.path().join("tracked.txt"), "v2");
    let (_, res3) = RustAdapter::build(&opt).expect("rebuild after file change");
    assert!(res3.success);
    assert!(
        !res3.cached,
        "must invalidate and recompile when tracked file changes"
    );
    let out3 = Command::new(&bin).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out3.stdout), "file=v2,env=e1");

    // Warm hit after file update
    let (_, res4) = RustAdapter::build(&opt).expect("warm build 2");
    assert!(res4.success);
    assert!(res4.cached);

    // Tracked env var change: invalidates build script and dependent unit
    let _env2 = EnvGuard::set("TEST_LAZY_ENV_VAR", "e2");
    let (_, res5) = RustAdapter::build(&opt).expect("rebuild after env change");
    assert!(res5.success);
    assert!(
        !res5.cached,
        "must invalidate and recompile when tracked env var changes"
    );
    let out5 = Command::new(&bin).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out5.stdout), "file=v2,env=e2");
}

#[test]
fn test_mutable_consumer_out_dir_remains_usable_on_hit() {
    let _lock = SERIAL_LOCK.lock().unwrap();
    let cache_dir = TempDir::new().unwrap();
    let ws = TempDir::new().unwrap();

    write_file(
        ws.path().join("Cargo.toml"),
        "[workspace]\nmembers = [\"producer_lib\", \"consumer_app\"]\nresolver = \"2\"\n",
    );
    write_file(
        ws.path().join("producer_lib/Cargo.toml"),
        "[package]\nname = \"producer_lib\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write_file(
        ws.path().join("producer_lib/build.rs"),
        r##"use std::{env, fs, path::Path};
fn main() {
    let out = env::var("OUT_DIR").unwrap();
    fs::write(Path::new(&out).join("data.txt"), "shared producer payload").unwrap();
    println!("cargo:rerun-if-changed=build.rs");
}
"##,
    );
    write_file(
        ws.path().join("producer_lib/src/lib.rs"),
        "pub fn read_payload() -> &'static str { include_str!(concat!(env!(\"OUT_DIR\"), \"/data.txt\")) }\n",
    );
    write_file(
        ws.path().join("consumer_app/Cargo.toml"),
        "[package]\nname = \"consumer_app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nproducer_lib = { path = \"../producer_lib\" }\n",
    );
    write_file(
        ws.path().join("consumer_app/src/main.rs"),
        "fn main() { print!(\"app:{}\", producer_lib::read_payload()); }\n",
    );

    let opt = RustBuildOptions {
        manifest_path: Some(ws.path().join("Cargo.toml")),
        cache_dir: Some(cache_dir.path().to_path_buf()),
        target_dir: Some(ws.path().join("target")),
        ..Default::default()
    };

    // Cold build
    let (_, res1) = RustAdapter::build(&opt).expect("cold build");
    assert!(res1.success);
    assert!(!res1.cached);
    let app_bin = ws.path().join("target/debug/consumer_app");
    let out1 = Command::new(&app_bin).output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out1.stdout),
        "app:shared producer payload"
    );

    // Warm build: both producer_lib and consumer_app hit cache
    let (_, res2) = RustAdapter::build(&opt).expect("warm build");
    assert!(res2.success);
    assert!(res2.cached);

    // Modify only consumer_app (consumer unit), leaving producer_lib untouched
    write_file(
        ws.path().join("consumer_app/src/main.rs"),
        "fn main() { print!(\"app_modified:{}\", producer_lib::read_payload()); }\n",
    );

    // Rebuild: producer_lib must hit cache without staging, and consumer_app
    // must compile cleanly reading producer_lib's cached OUT_DIR
    let (_, res3) = RustAdapter::build(&opt).expect("rebuild after consumer change");
    assert!(res3.success, "rebuild failed: {}", res3.stderr);

    let out3 = Command::new(&app_bin).output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out3.stdout),
        "app_modified:shared producer payload"
    );
}

#[test]
fn test_cached_build_script_still_runs_clippy() {
    let _lock = SERIAL_LOCK.lock().unwrap();
    let cache = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    write_file(
        root.path().join("Cargo.toml"),
        "[package]\nname = \"cached_clippy\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write_file(
        root.path().join("src/lib.rs"),
        "pub fn value() -> u32 { 1 }\n",
    );
    write_file(
        root.path().join("build.rs"),
        "fn main() { let _value = 1u32 as u32; println!(\"cargo:rerun-if-changed=build.rs\"); }\n",
    );
    let opt = RustBuildOptions {
        manifest_path: Some(root.path().join("Cargo.toml")),
        target_dir: Some(root.path().join("target")),
        cache_dir: Some(cache.path().to_path_buf()),
        ..Default::default()
    };
    let (_, built) = RustAdapter::build(&opt).unwrap();
    assert!(built.success, "{}", built.stderr);
    let lint = RustBuildOptions {
        mode: mitts::adapter::BuildMode::Clippy,
        extra_args: vec!["-Dclippy::unnecessary_cast".to_string()],
        ..opt
    };
    let diagnostics = match RustAdapter::build(&lint) {
        Ok((_, result)) => {
            assert!(!result.success, "cached build.rs must still be linted");
            result.stderr
        }
        Err(error) => format!("{error:#}"),
    };
    assert!(diagnostics.contains("unnecessary_cast"), "{diagnostics}");
}
