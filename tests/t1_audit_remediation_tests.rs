/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::rust::build_script::{
    compute_build_script_key, execute_build_script, BuildScriptDirectives,
};
use mitts::adapter::rust::build_script_cas::{remap_out_dir, remove_dir_all_readonly_safe};
use mitts::adapter::rust::build_script_purity::{
    assess_build_script_purity, assess_build_script_purity_with_dep_info,
    extract_out_dir_files_from_dep_info,
};
use mitts::adapter::rust::dag::CompilationDag;
use mitts::adapter::rust::dag_cache::{try_restore_from_cache_with_directives, ExecutionContext};
use mitts::adapter::rust::dag_unit_publish::taint_unhashable_key;
use mitts::adapter::rust::pipelining::{classify_dep_requirement, DepRequirement};
use mitts::adapter::rust::types::{
    EffectivePlanExplanation, PlannedCompilationUnit, RustBuildOptions,
};
use mitts::adapter::rust::RustAdapter;
use mitts::core::action::ToolchainInfo;
use mitts::core::action_cache::{ActionCache, ActionCacheEntry, CachedOutput};
use mitts::core::cas::ContentAddressedStore;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tempfile::TempDir;

fn dummy_toolchain() -> ToolchainInfo {
    ToolchainInfo {
        name: "rustc".to_string(),
        version: "1.85.0".to_string(),
        target_triple: Some("x86_64-unknown-linux-gnu".to_string()),
    }
}

fn make_lib_unit(
    package_id: &str,
    name: &str,
    version: &str,
    for_host: bool,
    dependencies: Vec<String>,
) -> PlannedCompilationUnit {
    PlannedCompilationUnit {
        package_id: package_id.to_string(),
        package_name: name.to_string(),
        version: version.to_string(),
        target_name: name.to_string(),
        kind: "lib".to_string(),
        for_host,
        dependencies,
        ..Default::default()
    }
}

#[test]
fn test_b1_pipelining_build_script_waits_for_transitive_host_deps_and_aliases() {
    let tmp = TempDir::new().unwrap();
    let l_dir = tmp.path().join("pkg_l");
    fs::create_dir_all(l_dir.join("src")).unwrap();
    fs::write(
        l_dir.join("Cargo.toml"),
        "[package]\nname = \"pkg_l\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[dependencies]\nnormal_dep = \"0.1.0\"\n[build-dependencies]\nmy_cc = { package = \"cc\", version = \"1.0\" }\n",
    )
    .unwrap();
    fs::write(l_dir.join("build.rs"), "fn main() {}\n").unwrap();
    fs::write(l_dir.join("src/lib.rs"), "pub fn l() {}\n").unwrap();

    let mut unit_l = make_lib_unit(
        "pkg_l@0.1.0",
        "pkg_l",
        "0.1.0",
        false,
        vec![
            "cc@1.0.0::lib:cc:host".to_string(),
            "normal_dep@0.1.0::lib:normal_dep".to_string(),
        ],
    );
    unit_l.src_path = l_dir.join("src/lib.rs");
    unit_l.manifest_path = l_dir.join("Cargo.toml");
    let unit_cc_host = make_lib_unit(
        "cc@1.0.0",
        "cc",
        "1.0.0",
        true,
        vec!["shlex@1.3.0::lib:shlex:host".to_string()],
    );
    let unit_shlex_host = make_lib_unit("shlex@1.3.0", "shlex", "1.3.0", true, Vec::new());
    let unit_cc_unhosted = make_lib_unit("cc@1.0.0", "cc", "1.0.0", false, Vec::new());
    let unit_normal = make_lib_unit("normal_dep@0.1.0", "normal_dep", "0.1.0", false, Vec::new());

    assert_eq!(
        classify_dep_requirement(&unit_l, &unit_shlex_host),
        DepRequirement::FullArtifact
    );
    assert_eq!(
        classify_dep_requirement(&unit_l, &unit_cc_unhosted),
        DepRequirement::FullArtifact
    );
    assert_eq!(
        classify_dep_requirement(&unit_l, &unit_normal),
        DepRequirement::MetadataOnly
    );

    let explanation = EffectivePlanExplanation {
        manifest_path: l_dir.join("Cargo.toml"),
        target_directory: tmp.path().join("target"),
        profile: "dev".to_string(),
        toolchain: dummy_toolchain(),
        units: vec![
            unit_shlex_host.clone(),
            unit_cc_host,
            unit_normal,
            unit_l.clone(),
        ],
    };
    let dag = CompilationDag::from_explanation(&explanation).unwrap();
    let l_node = dag.get_node(&unit_l.unit_id()).unwrap();
    assert!(l_node.dependencies.contains(&unit_shlex_host.unit_id()));
    let shlex_node = dag.get_node(&unit_shlex_host.unit_id()).unwrap();
    assert!(shlex_node.dependents.contains(&unit_l.unit_id()));
    assert_eq!(
        classify_dep_requirement(&l_node.unit, &shlex_node.unit),
        DepRequirement::FullArtifact
    );
}

#[test]
fn test_b2_dep_links_metadata_and_link_search_subdir_remap_across_warm_build() {
    let mut d = BuildScriptDirectives {
        link_searches: vec![
            "/tmp/stage_a/out".to_string(),
            "native=/tmp/stage_a/out/lib".to_string(),
        ],
        envs: HashMap::from([("GEN_HEADER".to_string(), "/tmp/stage_a/out/inc".to_string())]),
        metadata: HashMap::from([(
            "include".to_string(),
            "/tmp/stage_a/out/include".to_string(),
        )]),
        links: Some("a".to_string()),
        ..Default::default()
    };
    remap_out_dir(&mut d, "/tmp/stage_a/out", "/cas/build_scripts/k/e/out");
    assert_eq!(
        d.link_searches,
        vec![
            "/cas/build_scripts/k/e/out".to_string(),
            "native=/cas/build_scripts/k/e/out/lib".to_string()
        ]
    );
    assert_eq!(
        d.envs.get("GEN_HEADER").map(String::as_str),
        Some("/cas/build_scripts/k/e/out/inc")
    );
    assert_eq!(
        d.dep_env_vars().get("DEP_A_INCLUDE").map(String::as_str),
        Some("/cas/build_scripts/k/e/out/include")
    );

    let tmp = TempDir::new().unwrap();
    let cache_dir = tmp.path().join("cache");
    let ws = tmp.path().join("ws");
    let dir_a = ws.join("a");
    let dir_b = ws.join("b");
    fs::create_dir_all(dir_a.join("src")).unwrap();
    fs::create_dir_all(dir_b.join("src")).unwrap();
    fs::write(
        ws.join("Cargo.toml"),
        "[workspace]\nmembers = [\"a\", \"b\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    fs::write(
        dir_a.join("Cargo.toml"),
        "[package]\nname = \"a\"\nversion = \"0.1.0\"\nedition = \"2021\"\nlinks = \"a\"\n",
    )
    .unwrap();
    fs::write(
        dir_a.join("build.rs"),
        "use std::{env, fs, path::PathBuf};\nfn main() {\n    let out = PathBuf::from(env::var(\"OUT_DIR\").unwrap());\n    let inc = out.join(\"include\");\n    fs::create_dir_all(&inc).unwrap();\n    fs::write(inc.join(\"a.h\"), \"/* header */\\n\").unwrap();\n    println!(\"cargo:include={}\", inc.display());\n    println!(\"cargo:rustc-link-search=native={}\", inc.display());\n}\n",
    )
    .unwrap();
    fs::write(dir_a.join("src/lib.rs"), "pub fn a() -> u32 { 1 }\n").unwrap();

    fs::write(
        dir_b.join("Cargo.toml"),
        "[package]\nname = \"b\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[dependencies]\na = { path = \"../a\" }\n",
    )
    .unwrap();
    fs::write(
        dir_b.join("build.rs"),
        "use std::{env, path::Path};\nfn main() {\n    let inc = env::var(\"DEP_A_INCLUDE\").expect(\"DEP_A_INCLUDE missing\");\n    assert!(Path::new(&inc).join(\"a.h\").is_file(), \"missing a.h at {inc}\");\n}\n",
    )
    .unwrap();
    fs::write(
        dir_b.join("src/lib.rs"),
        "pub fn b() -> u32 { a::a() + 1 }\n",
    )
    .unwrap();
    fs::write(
        ws.join("Cargo.lock"),
        "version = 3\n[[package]]\nname = \"a\"\nversion = \"0.1.0\"\n[[package]]\nname = \"b\"\nversion = \"0.1.0\"\ndependencies = [\"a\"]\n",
    )
    .unwrap();

    let opt = RustBuildOptions {
        manifest_path: Some(ws.join("Cargo.toml")),
        native: true,
        cache_dir: Some(cache_dir.clone()),
        ..Default::default()
    };
    let (_, res1) = RustAdapter::build_native(&opt).unwrap();
    assert!(res1.success, "cold build failed: {}", res1.stderr);

    // 1. Clear ActionCache and modify b's build.rs so `a` restores from `cas/build_scripts/<key_a>`
    let _ = fs::remove_dir_all(cache_dir.join("ac"));
    fs::write(
        dir_b.join("build.rs"),
        "use std::{env, path::Path};\nfn main() {\n    // warm run 1 (from build_script_cas)\n    let inc = env::var(\"DEP_A_INCLUDE\").expect(\"DEP_A_INCLUDE missing\");\n    assert!(Path::new(&inc).join(\"a.h\").is_file(), \"missing a.h at {inc}\");\n}\n",
    )
    .unwrap();
    let (_, res2) = RustAdapter::build_native(&opt).unwrap();
    assert!(
        res2.success,
        "warm build (build_script_cas) failed: {}",
        res2.stderr
    );

    // 2. Keep ActionCache intact and modify b's build.rs so `a` restores from ActionCache + metadata.toml
    fs::write(
        dir_b.join("build.rs"),
        "use std::{env, path::Path};\nfn main() {\n    // warm run 2 (from ActionCache)\n    let inc = env::var(\"DEP_A_INCLUDE\").expect(\"DEP_A_INCLUDE missing\");\n    assert!(Path::new(&inc).join(\"a.h\").is_file(), \"missing a.h at {inc}\");\n}\n",
    )
    .unwrap();
    let (_, res3) = RustAdapter::build_native(&opt).unwrap();
    assert!(
        res3.success,
        "warm build (ActionCache) failed: {}",
        res3.stderr
    );
    let _ = remove_dir_all_readonly_safe(&cache_dir);
}

#[test]
fn test_b3_build_script_rejects_untracked_global_cas_rlib_scan() {
    let tmp = TempDir::new().unwrap();
    let pkg_dir = tmp.path().join("pkg");
    let staging_dir = tmp.path().join("staging");
    let cas_artifacts = tmp.path().join("cas").join("artifacts");
    fs::create_dir_all(pkg_dir.join("src")).unwrap();
    fs::create_dir_all(&staging_dir).unwrap();
    fs::create_dir_all(&cas_artifacts).unwrap();

    fs::write(cas_artifacts.join("librogue_dep-123456.rlib"), b"rogue").unwrap();
    fs::write(
        pkg_dir.join("Cargo.toml"),
        "[package]\nname = \"pkg\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[build-dependencies]\nrogue-dep = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(pkg_dir.join("build.rs"), "fn main() {}\n").unwrap();
    fs::write(pkg_dir.join("src/lib.rs"), "pub fn f() {}\n").unwrap();

    let mut unit = make_lib_unit("pkg@0.1.0", "pkg", "0.1.0", false, Vec::new());
    unit.src_path = pkg_dir.join("src/lib.rs");
    unit.manifest_path = pkg_dir.join("Cargo.toml");

    let err = execute_build_script(
        &unit,
        &staging_dir,
        false,
        &[],
        Some(&cas_artifacts),
        &[],
        None,
    )
    .expect_err("must reject missing active build-dependency instead of scanning CAS");
    assert!(
        format!("{:#}", err).contains("Active build-dependency 'rogue-dep'"),
        "unexpected error: {err:#}"
    );
}

#[test]
fn test_b5_tainted_key_uses_random_nonce_and_blocks_cache_restore_and_long_ac_put() {
    let k1 = taint_unhashable_key("base_action_key");
    let k2 = taint_unhashable_key("base_action_key");
    assert!(k1.contains(":unhashable:"));
    assert_ne!(
        k1, k2,
        "tainted keys must use unique nonces, not static PID"
    );

    let tmp = TempDir::new().unwrap();
    let cas = ContentAddressedStore::new(tmp.path().join("cas")).unwrap();
    let ac = ActionCache::new(tmp.path().join("ac")).unwrap();

    let (h, sz) = cas.store_bytes(b"rlib_bytes").unwrap();
    let long_key = format!(
        "{}:bd:{}:ws:{}:ws_leak:{}",
        "a".repeat(64),
        "b".repeat(64),
        "c".repeat(64),
        "d".repeat(64)
    );
    assert!(long_key.len() > 260);
    let entry = ActionCacheEntry {
        action_key: long_key.clone(),
        action_id: "u1".to_string(),
        created_at_secs: 1,
        outputs: vec![CachedOutput {
            relative_path: PathBuf::from("libfoo.rlib"),
            cas_hash: h.clone(),
            file_size: sz,
            executable: false,
        }],
        stdout: String::new(),
        stderr: String::new(),
        exit_code: 0,
    };
    ac.put(&entry)
        .expect("ActionCache::put must handle keys >255 chars");
    assert!(ac.get(&long_key).unwrap().is_some());

    let pkg_dir = tmp.path().join("pkg");
    fs::create_dir_all(pkg_dir.join("src")).unwrap();
    fs::write(
        pkg_dir.join("Cargo.toml"),
        "[package]\nname = \"foo\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    let mut unit = make_lib_unit("foo@0.1.0", "foo", "0.1.0", false, Vec::new());
    unit.manifest_path = pkg_dir.join("Cargo.toml");
    let (completed_artifacts, recorded_actions, compiler_messages, any_missed) = (
        Arc::new(Mutex::new(HashMap::new())),
        Arc::new(Mutex::new(Vec::new())),
        Arc::new(Mutex::new(Vec::new())),
        Arc::new(Mutex::new(false)),
    );
    let (dep_build_metadata, resolved_keys) = (
        Arc::new(Mutex::new(HashMap::new())),
        Arc::new(Mutex::new(HashMap::new())),
    );
    let (opt, tc) = (RustBuildOptions::default(), dummy_toolchain());
    let ctx = ExecutionContext {
        opt: &opt,
        toolchain: &tc,
        target_dir: &tmp.path().join("target"),
        cas: &cas,
        ac: &ac,
        is_cache_enabled: true,
        completed_artifacts: &completed_artifacts,
        recorded_actions: &recorded_actions,
        compiler_messages: &compiler_messages,
        any_missed: &any_missed,
        dep_build_metadata: &dep_build_metadata,
        resolved_keys: Some(&resolved_keys),
        jobserver: None,
        is_first_job: None,
        on_metadata_ready: None,
        compiled_actions: None,
        cached_actions: None,
        completed_cfgs: None,
        completed_env: None,
        completed_link_args: None,
        target_index: None,
        tracked_extra_files: None,
        tracked_env_vars: None,
    };

    assert!(!try_restore_from_cache_with_directives(&unit, &k1, &ctx, false, None).unwrap());
    assert!(resolved_keys.lock().unwrap().is_empty());

    let obj_path = cas.object_path(&h);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&obj_path).unwrap().permissions();
        perms.set_mode(0o644);
        fs::set_permissions(&obj_path, perms).unwrap();
    }
    fs::remove_file(&obj_path).unwrap();
    assert!(!try_restore_from_cache_with_directives(&unit, &long_key, &ctx, false, None).unwrap());
    assert!(resolved_keys.lock().unwrap().is_empty());
}

#[test]
fn test_b6_non_registry_build_script_key_and_n2_cas_tmp_purity() {
    let tmp = TempDir::new().unwrap();
    let git_pkg = tmp.path().join("sources").join("git_dep");
    fs::create_dir_all(git_pkg.join("proto")).unwrap();
    fs::create_dir_all(git_pkg.join("src")).unwrap();
    fs::write(
        git_pkg.join("Cargo.toml"),
        "[package]\nname = \"git_dep\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    let build_rs = git_pkg.join("build.rs");
    fs::write(&build_rs, "fn main() {}\n").unwrap();
    fs::write(git_pkg.join("src/lib.rs"), "pub fn g() {}\n").unwrap();
    fs::write(git_pkg.join("proto/api.proto"), "syntax = \"proto2\";\n").unwrap();

    let mut unit = make_lib_unit(
        "git_dep@0.1.0 (git+https://example.com/repo.git#1111)",
        "git_dep",
        "0.1.0",
        false,
        Vec::new(),
    );
    unit.src_path = git_pkg.join("src/lib.rs");
    unit.manifest_path = git_pkg.join("Cargo.toml");
    let k1 = compute_build_script_key(
        &unit,
        &build_rs,
        false,
        &[],
        "x86_64-unknown-linux-gnu",
        None,
        &[],
    )
    .unwrap();
    fs::write(git_pkg.join("proto/api.proto"), "syntax = \"proto3\";\n").unwrap();
    let k2 = compute_build_script_key(
        &unit,
        &build_rs,
        false,
        &[],
        "x86_64-unknown-linux-gnu",
        None,
        &[],
    )
    .unwrap();
    assert_ne!(k1, k2);

    // R1: OUT_DIR contains probe.d with staging path and gen.rs without paths,
    // and unit dep-info includes gen.rs -> unit is pure; if gen.rs has staging path -> impure.
    let cas_root = tmp.path().join("cas");
    let cas_out = cas_root
        .join("build_scripts")
        .join("k")
        .join("e")
        .join("out");
    let unit_stage = cas_root.join("tmp").join("stage_git_dep_1_1");
    fs::create_dir_all(&cas_out).unwrap();
    fs::create_dir_all(&unit_stage).unwrap();
    let leaked_stage = unit_stage.join("out");
    fs::write(
        cas_out.join("probe.d"),
        format!(
            "{}/probe.rmeta: {}/probe.rs\n",
            leaked_stage.display(),
            leaked_stage.display()
        ),
    )
    .unwrap();
    let gen_rs = cas_out.join("gen.rs");
    fs::write(&gen_rs, "pub fn val() -> u32 { 42 }\n").unwrap();
    fs::write(
        unit_stage.join("git_dep.d"),
        format!(
            "{}/libgit_dep.rlib: {} {}\n",
            unit_stage.display(),
            git_pkg.join("src/lib.rs").display(),
            gen_rs.display()
        ),
    )
    .unwrap();
    let d = BuildScriptDirectives {
        out_dir: Some(cas_out.clone()),
        ..Default::default()
    };
    let refs = extract_out_dir_files_from_dep_info(&unit_stage, &cas_out, "git_dep", None)
        .expect("must parse unit dep-info");
    assert!(refs.contains("gen.rs") && !refs.contains("probe.d"));
    let purity_clean = assess_build_script_purity_with_dep_info(
        &git_pkg,
        None,
        &cas_out,
        None,
        Some(&cas_root),
        &d,
        Some(&refs),
    )
    .unwrap();
    assert!(
        purity_clean.is_pure,
        "probe.d with staging path must not taint unit when only clean gen.rs is in unit .d"
    );

    fs::write(
        &gen_rs,
        format!("pub const STAGE: &str = \"{}\";\n", leaked_stage.display()),
    )
    .unwrap();
    let purity_leaked = assess_build_script_purity_with_dep_info(
        &git_pkg,
        None,
        &cas_out,
        None,
        Some(&cas_root),
        &d,
        Some(&refs),
    )
    .unwrap();
    assert!(
        !purity_leaked.is_pure,
        "gen.rs included by unit .d with staging path must mark unit impure"
    );
    let purity_fallback =
        assess_build_script_purity(&git_pkg, None, &cas_out, None, Some(&cas_root), &d).unwrap();
    assert!(!purity_fallback.is_pure);
}

#[test]
fn test_r1_cross_clone_cache_hit_with_probe_dep_info_vs_impure_gen_rs_and_readonly_cas() {
    let tmp = TempDir::new().unwrap();
    let cache_dir = tmp.path().join("cache");
    let setup_clone = |clone_root: &std::path::Path, leak_in_gen_rs: bool| {
        let dep_dir = clone_root.join("vendor").join("probe_dep");
        let app_dir = clone_root.join("ws");
        fs::create_dir_all(dep_dir.join("src")).unwrap();
        fs::create_dir_all(app_dir.join("src")).unwrap();
        fs::write(
            dep_dir.join("Cargo.toml"),
            "[package]\nname = \"probe_dep\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        let gen_body = if leak_in_gen_rs {
            "format!(\"pub fn msg() -> &'static str {{ {:?} }}\\n\", out)"
        } else {
            "\"pub fn msg() -> &'static str { \\\"pure\\\" }\\n\".to_string()"
        };
        fs::write(
            dep_dir.join("build.rs"),
            format!(
                "use std::{{env, fs, path::PathBuf}};\nfn main() {{\n    let out = PathBuf::from(env::var(\"OUT_DIR\").unwrap());\n    fs::write(out.join(\"probe.d\"), format!(\"{{}}/probe: {{}}/probe.rs\\n\", out.display(), out.display())).unwrap();\n    fs::write(out.join(\"gen.rs\"), {gen_body}).unwrap();\n}}\n"
            ),
        )
        .unwrap();
        fs::write(
            dep_dir.join("src/lib.rs"),
            "include!(concat!(env!(\"OUT_DIR\"), \"/gen.rs\"));\n",
        )
        .unwrap();
        fs::write(
            app_dir.join("Cargo.toml"),
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[dependencies]\nprobe_dep = { path = \"../vendor/probe_dep\" }\n",
        )
        .unwrap();
        fs::write(
            app_dir.join("src/lib.rs"),
            "pub fn run() -> &'static str { probe_dep::msg() }\n",
        )
        .unwrap();
        app_dir
    };

    // Case 1: probe.d has staging path, gen.rs is clean -> Clone 2 hits cache (compiled = 0)
    let app1 = setup_clone(&tmp.path().join("clone1"), false);
    let app2 = setup_clone(&tmp.path().join("clone2"), false);
    let (_, r1) = RustAdapter::build_native(&RustBuildOptions {
        manifest_path: Some(app1.join("Cargo.toml")),
        native: true,
        cache_dir: Some(cache_dir.clone()),
        ..Default::default()
    })
    .unwrap();
    assert!(
        r1.success && !r1.cached,
        "clone1 cold build failed: {}",
        r1.stderr
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let bs_root = cache_dir.join("cas").join("build_scripts");
        let mut checked_perms = false;
        for k in fs::read_dir(&bs_root).unwrap().flatten() {
            for e in fs::read_dir(k.path()).unwrap().flatten() {
                let out = e.path().join("debug/build/probe_dep/out");
                if out.is_dir() {
                    let dir_mode = fs::metadata(&out).unwrap().permissions().mode() & 0o777;
                    let file_mode = fs::metadata(out.join("gen.rs"))
                        .unwrap()
                        .permissions()
                        .mode()
                        & 0o777;
                    assert_eq!(dir_mode, 0o555, "CAS OUT_DIR directory must be 0o555");
                    assert_eq!(file_mode, 0o444, "CAS OUT_DIR file must be 0o444");
                    checked_perms = true;
                }
            }
        }
        assert!(
            checked_perms,
            "must verify read-only permissions in cas/build_scripts"
        );
    }

    let (_, r2) = RustAdapter::build_native(&RustBuildOptions {
        manifest_path: Some(app2.join("Cargo.toml")),
        native: true,
        cache_dir: Some(cache_dir.clone()),
        ..Default::default()
    })
    .unwrap();
    assert!(
        r2.success && r2.cached,
        "clone2 warm build must be 100% cache hit when only probe.d has staging path: {}",
        r2.stderr
    );

    // Case 2: gen.rs itself contains staging path -> Clone 4 misses cache
    let cache_dir_impure = tmp.path().join("cache_impure");
    let app3 = setup_clone(&tmp.path().join("clone3"), true);
    let app4 = setup_clone(&tmp.path().join("clone4"), true);
    let (_, r3) = RustAdapter::build_native(&RustBuildOptions {
        manifest_path: Some(app3.join("Cargo.toml")),
        native: true,
        cache_dir: Some(cache_dir_impure.clone()),
        ..Default::default()
    })
    .unwrap();
    assert!(r3.success && !r3.cached);
    let (_, r4) = RustAdapter::build_native(&RustBuildOptions {
        manifest_path: Some(app4.join("Cargo.toml")),
        native: true,
        cache_dir: Some(cache_dir_impure.clone()),
        ..Default::default()
    })
    .unwrap();
    assert!(
        r4.success && !r4.cached,
        "clone4 must miss cache when included gen.rs contains staging path"
    );

    let _ = remove_dir_all_readonly_safe(&cache_dir);
    let _ = remove_dir_all_readonly_safe(&cache_dir_impure);
}
