/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::rust::source_lookup::find_crate_source_dir;
use std::fs;
use std::sync::Mutex;
use tempfile::TempDir;

static ENV_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn test_m8_source_lookup_git_and_cargo_flow() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let temp = TempDir::new().unwrap();
    let home = temp.path();
    let old_home = std::env::var_os("HOME");
    let old_ch = std::env::var_os("CARGO_HOME");
    std::env::remove_var("CARGO_HOME");
    std::env::set_var("HOME", home);

    let checkouts = home.join(".cargo").join("git").join("checkouts");
    let repo_dir = checkouts.join("my-repo-1234");
    let commit_dir = repo_dir.join("a1b2c3d4e5f6");
    fs::create_dir_all(&commit_dir).unwrap();

    fs::create_dir_all(commit_dir.join(".git")).unwrap();
    fs::create_dir_all(commit_dir.join("target")).unwrap();

    let sub_pkg = commit_dir.join("sub_pkg");
    fs::create_dir_all(&sub_pkg).unwrap();
    let sub_toml = "[package]\nname = \"sub_pkg\"\nversion = \"0.1.0\"\n";
    fs::write(sub_pkg.join("Cargo.toml"), sub_toml).unwrap();

    let crates_dir = commit_dir.join("crates").join("nested_pkg");
    fs::create_dir_all(&crates_dir).unwrap();
    let nested_toml = "[package]\nname = \"nested_pkg\"\nversion = \"0.2.0\"\n";
    fs::write(crates_dir.join("Cargo.toml"), nested_toml).unwrap();

    let source_url = "git+https://github.com/foo/bar?branch=main#a1b2c3d4";
    assert_eq!(
        find_crate_source_dir("sub_pkg", "0.1.0", Some(source_url)),
        Some(sub_pkg)
    );
    assert_eq!(
        find_crate_source_dir("nested_pkg", "0.2.0", Some(source_url)),
        Some(crates_dir)
    );

    let root_commit = checkouts.join("root-repo-5678/c0ffee012345");
    fs::create_dir_all(&root_commit).unwrap();
    let root_toml = "[package]\nname = \"root_pkg\"\nversion = \"1.0.0\"\n";
    fs::write(root_commit.join("Cargo.toml"), root_toml).unwrap();

    let root_source = "git+https://github.com/foo/root#c0ffee";
    assert_eq!(
        find_crate_source_dir("root_pkg", "1.0.0", Some(root_source)),
        Some(root_commit)
    );
    assert_eq!(find_crate_source_dir("root_pkg", "1.0.0", None), None);
    let bad_hash = "git+https://github.com/foo/bar?branch=main#ffffffff";
    assert!(find_crate_source_dir("sub_pkg", "0.1.0", Some(bad_hash)).is_none());

    let reg_dir =
        home.join(".cargo/registry/src/index.crates.io-6f17d22bba15001f/serde_json-1.0.100");
    fs::create_dir_all(&reg_dir).unwrap();
    fs::write(
        reg_dir.join("Cargo.toml"),
        "[package]\nname = \"serde_json\"\n",
    )
    .unwrap();
    fs::write(reg_dir.join(".cargo-ok"), "").unwrap();
    let crates_io = Some("registry+https://github.com/rust-lang/crates.io-index");
    assert_eq!(
        find_crate_source_dir("serde_json", "1.0.100", crates_io),
        Some(reg_dir)
    );

    // 6. Mitts cache source lookup (requires .mitts-ok)
    let mitts_dir = home.join(".cache/mitts/sources/tokio-1.30.0");
    fs::create_dir_all(&mitts_dir).unwrap();
    fs::write(
        mitts_dir.join("Cargo.toml"),
        "[package]\nname = \"tokio\"\n",
    )
    .unwrap();
    fs::write(mitts_dir.join(".mitts-ok"), "ok\n").unwrap();
    assert_eq!(
        find_crate_source_dir("tokio", "1.30.0", crates_io),
        Some(mitts_dir)
    );

    // 7. Missing checkouts directory
    let empty_home = temp.path().join("empty_home");
    fs::create_dir_all(&empty_home).unwrap();
    std::env::set_var("HOME", &empty_home);
    assert!(find_crate_source_dir(
        "any_crate",
        "1.0.0",
        Some("git+https://github.com/test/repo")
    )
    .is_none());
    assert!(find_crate_source_dir("any_crate", "1.0.0", None).is_none());

    if let Some(ch) = old_ch {
        std::env::set_var("CARGO_HOME", ch);
    }
    if let Some(h) = old_home {
        std::env::set_var("HOME", h);
    }
}

#[test]
fn test_t0_g1_git_checkouts_ignores_non_dir_files_and_verifies_package_name_version() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let temp = TempDir::new().unwrap();
    let home = temp.path();
    let old_home = std::env::var_os("HOME");
    let old_ch = std::env::var_os("CARGO_HOME");
    std::env::remove_var("CARGO_HOME");
    std::env::set_var("HOME", home);

    let checkouts = home.join(".cargo/git/checkouts");
    fs::create_dir_all(&checkouts).unwrap();
    fs::write(checkouts.join(".DS_Store"), b"junk").unwrap();
    fs::write(checkouts.join("0000_non_dir_entry"), b"junk").unwrap();

    let commit_dir = checkouts.join("zzzz-repo-9999/a1b2c3d4e5f6");
    fs::create_dir_all(&commit_dir).unwrap();

    let decoy_bin = commit_dir.join("decoy_bin");
    fs::create_dir_all(&decoy_bin).unwrap();
    fs::write(
        decoy_bin.join("Cargo.toml"),
        "[package]\nname = \"other_pkg\"\nversion = \"0.2.0\"\n\n[[bin]]\nname = \"target_pkg\"\npath = \"src/main.rs\"\n",
    )
    .unwrap();

    let real_pkg = commit_dir.join("real_pkg");
    let no_ver_pkg = commit_dir.join("no_ver_pkg");
    fs::create_dir_all(&real_pkg).unwrap();
    fs::create_dir_all(&no_ver_pkg).unwrap();
    fs::write(
        real_pkg.join("Cargo.toml"),
        "[package]\nname = \"target_pkg\"\nversion = \"0.2.0\"\n",
    )
    .unwrap();
    fs::write(
        no_ver_pkg.join("Cargo.toml"),
        "[package]\nname = \"vless\"\n",
    )
    .unwrap();

    let git_src = "git+https://github.com/example/repo#a1b2c3d4e5f6";
    assert_eq!(
        find_crate_source_dir("target_pkg", "0.2.0", Some(git_src)),
        Some(real_pkg)
    );
    assert_eq!(
        find_crate_source_dir("vless", "0.0.0", Some(git_src)),
        Some(no_ver_pkg)
    );
    assert_eq!(
        find_crate_source_dir("target_pkg", "0.9.9", Some(git_src)),
        None
    );

    if let Some(ch) = old_ch {
        std::env::set_var("CARGO_HOME", ch);
    }
    if let Some(h) = old_home {
        std::env::set_var("HOME", h);
    }
}

#[test]
fn test_t0_b5_single_rustc_source_of_truth_and_0_0_0_path_version() {
    use mitts::adapter::rust::build_script_keys::get_rustc_verbose_version;
    use mitts::adapter::rust::compiler_resolve::{
        active_toolchain, resolve_host_triple, resolve_rustc, rustc_vv_call_count, toolchain_for,
        try_resolve_host_triple,
    };
    use mitts::adapter::rust::types::RustBuildOptions;
    use mitts::adapter::RustAdapter;
    use mitts::core::build_lock::{
        FastBuildLock, FreshnessCheckArgs, FreshnessResult, RecordStateArgs,
    };

    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let temp = TempDir::new().unwrap();
    let home = temp.path().join("home");
    fs::create_dir_all(&home).unwrap();

    // 1. B5 invariant: resolve_rustc(), toolchain_for(&resolved), active_toolchain(),
    // detect_toolchain(), resolve_host_triple(), and get_rustc_verbose_version() share one -vV probe.
    let resolved_path = resolve_rustc();
    let probed = toolchain_for(&resolved_path).unwrap();
    let vv_before = rustc_vv_call_count();
    assert_eq!(probed.rustc_path, resolved_path);
    assert_eq!(active_toolchain().unwrap(), probed);
    let detected = RustAdapter::detect_toolchain().unwrap();
    assert_eq!(detected, probed.toolchain);
    assert_eq!(resolve_host_triple(), probed.host_triple);
    assert_eq!(get_rustc_verbose_version().unwrap(), probed.verbose_version);
    assert_eq!(rustc_vv_call_count(), vv_before);

    // 2. --manifest-path sub/Cargo.toml with sub/rust-toolchain.toml uses resolve_rustc() from cwd
    // and synthesizes 0.0.0 (not duplicate 0.1.0) for versionless path dependencies.
    let sub = temp.path().join("sub");
    let dep = sub.join("dep_no_ver");
    fs::create_dir_all(sub.join("src")).unwrap();
    fs::create_dir_all(dep.join("src")).unwrap();
    fs::write(
        sub.join("rust-toolchain.toml"),
        "[toolchain]\nchannel = \"nonexistent-toolchain-b5\"\n",
    )
    .unwrap();
    fs::write(
        dep.join("Cargo.toml"),
        "[package]\nname = \"dep_no_ver\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(dep.join("src/lib.rs"), "pub fn d() {}\n").unwrap();
    fs::write(
        sub.join("Cargo.toml"),
        "[package]\nname = \"sub_app\"\nedition = \"2021\"\n\n[dependencies]\ndep_no_ver = { path = \"dep_no_ver\" }\n",
    )
    .unwrap();
    fs::write(sub.join("src/lib.rs"), "pub fn s() {}\n").unwrap();
    fs::write(
        sub.join("Cargo.lock"),
        "version = 3\n\n[[package]]\nname = \"dep_no_ver\"\nversion = \"0.0.0\"\n\n[[package]]\nname = \"sub_app\"\nversion = \"0.0.0\"\ndependencies = [\n \"dep_no_ver\",\n]\n",
    )
    .unwrap();

    let opt = RustBuildOptions {
        manifest_path: Some(sub.join("Cargo.toml")),
        cache_dir: Some(temp.path().join("mitts_cache")),
        ..Default::default()
    };
    let plan = RustAdapter::explain_plan(&opt).unwrap();
    assert_eq!(plan.toolchain, detected);
    let dep_units: Vec<_> = plan
        .units
        .iter()
        .filter(|u| u.package_name == "dep_no_ver")
        .collect();
    assert_eq!(dep_units.len(), 1);
    assert_eq!(dep_units[0].version, "0.0.0");
    assert!(!home.join(".mitts/cache/toolchain_v1.json").exists());

    let out_rlib = sub.join("target/debug/libsub_app.rlib");
    fs::create_dir_all(out_rlib.parent().unwrap()).unwrap();
    fs::write(&out_rlib, b"rlib").unwrap();
    FastBuildLock::record_state(&RecordStateArgs {
        project_dir: &sub,
        profile: "dev",
        package: &None,
        features: &[],
        no_default_features: false,
        all_features: false,
        toolchain_version: &detected.version,
        action_count: 1,
        root_binary: None,
        outputs: &[out_rlib],
        stderr: None,
    })
    .unwrap();

    let check = |tc_ver: &str| {
        FastBuildLock::check_freshness(&FreshnessCheckArgs {
            project_dir: &sub,
            profile: "dev",
            package: &None,
            features: &[],
            no_default_features: false,
            all_features: false,
            toolchain_version: tc_ver,
        })
    };
    assert!(matches!(
        check(&detected.version),
        FreshnessResult::Fresh { .. }
    ));
    assert!(matches!(
        check("rustc 1.99.0-other"),
        FreshnessResult::Stale(_)
    ));

    // 3. B6 fail-closed: when RUSTC exits with code 1, detect_toolchain() and key derivation return Err.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let bad_rustc = temp.path().join("bad_rustc.sh");
        fs::write(
            &bad_rustc,
            "#!/bin/sh\necho 'missing toolchain' >&2\nexit 1\n",
        )
        .unwrap();
        fs::set_permissions(&bad_rustc, fs::Permissions::from_mode(0o755)).unwrap();
        let old_rustc = std::env::var_os("RUSTC");
        std::env::set_var("RUSTC", &bad_rustc);
        let tc_err = RustAdapter::detect_toolchain().expect_err("broken RUSTC must fail closed");
        assert!(format!("{:#}", tc_err).contains("missing toolchain"));
        assert!(try_resolve_host_triple().is_err());
        assert!(get_rustc_verbose_version().is_err());
        assert!(RustAdapter::explain_plan(&opt).is_err());
        assert!(RustAdapter::derive_unit_action_keys(&opt, &plan).is_err());
        match old_rustc {
            Some(v) => std::env::set_var("RUSTC", v),
            None => std::env::remove_var("RUSTC"),
        }
    }
}

#[test]
fn test_t0_r3_path_and_crates_io_same_name_resolved_by_source_and_version() {
    use mitts::adapter::rust::types::RustBuildOptions;
    use mitts::adapter::RustAdapter;
    use mitts::core::source_store::MITTS_OK_MARKER;

    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let temp = TempDir::new().unwrap();
    let mitts_cache = temp.path().join("mitts_cache");

    for (name, ver, dep) in [("foo", "1.0.0", ""), ("bar", "0.1.0", "foo = \"1.0.0\"\n")] {
        let dir = mitts_cache.join(format!("sources/{}-{}", name, ver));
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(
            dir.join("Cargo.toml"),
            format!("[package]\nname = \"{name}\"\nversion = \"{ver}\"\nedition = \"2021\"\n\n[dependencies]\n{dep}"),
        )
        .unwrap();
        fs::write(dir.join("src/lib.rs"), "pub fn f() {}\n").unwrap();
        fs::write(dir.join(MITTS_OK_MARKER), "ok\n").unwrap();
    }

    let proj = temp.path().join("proj");
    let foo_fork = proj.join("foo-fork");
    fs::create_dir_all(proj.join("src")).unwrap();
    fs::create_dir_all(foo_fork.join("src")).unwrap();
    fs::write(
        foo_fork.join("Cargo.toml"),
        "[package]\nname = \"foo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(foo_fork.join("src/lib.rs"), "pub fn local_fork() {}\n").unwrap();
    fs::write(
        proj.join("Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nfoo = { path = \"foo-fork\" }\nbar = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(proj.join("src/lib.rs"), "pub fn app() {}\n").unwrap();
    fs::write(
        proj.join("Cargo.lock"),
        "version = 3\n\n[[package]]\nname = \"app\"\nversion = \"0.1.0\"\ndependencies = [\n \"bar 0.1.0\",\n \"foo 0.1.0\",\n]\n\n[[package]]\nname = \"bar\"\nversion = \"0.1.0\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\ndependencies = [\n \"foo 1.0.0\",\n]\n\n[[package]]\nname = \"foo\"\nversion = \"0.1.0\"\n\n[[package]]\nname = \"foo\"\nversion = \"1.0.0\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\n",
    )
    .unwrap();

    let plan = RustAdapter::explain_plan(&RustBuildOptions {
        manifest_path: Some(proj.join("Cargo.toml")),
        cache_dir: Some(mitts_cache.clone()),
        ..Default::default()
    })
    .unwrap();

    let foo_01 = plan
        .units
        .iter()
        .find(|u| u.package_name == "foo" && u.version == "0.1.0")
        .unwrap();
    let foo_10 = plan
        .units
        .iter()
        .find(|u| u.package_name == "foo" && u.version == "1.0.0")
        .unwrap();
    assert_eq!(foo_01.manifest_path, foo_fork.join("Cargo.toml"));
    assert_eq!(
        foo_10.manifest_path,
        mitts_cache.join("sources/foo-1.0.0/Cargo.toml")
    );
}
