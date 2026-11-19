/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Feature sets Cargo computes and Mitts did not, found by comparing `mitts explain` with
//! `cargo build --unit-graph` over the corpus:
//! - resolver 1 resolves features through workspace members' dev-dependencies even for a
//!   plain build (tower's dev-dependency turns on tokio `macros`, so tokio-macros is built),
//!   while the dev-dependencies themselves are not built;
//! - `--all-features` turns on the implicit feature of every optional dependency too
//!   (tokio's optional `tracing`).

use mitts::adapter::rust::features::{
    resolve_package_features_with_resolver, PackageFeatureContext,
};
use mitts::adapter::rust::manifest::CargoToml;
use mitts::adapter::rust::types::RustBuildOptions;

fn ctx<'a>(
    name: &'a str,
    toml: &'a CargoToml,
    deps: &'a [String],
    root: bool,
) -> PackageFeatureContext<'a> {
    PackageFeatureContext {
        name,
        version: "1.0.0",
        source: None,
        is_workspace: root,
        is_root: root,
        toml,
        dependencies: deps,
    }
}

struct Graph {
    app: CargoToml,
    rt: CargoToml,
    macros: CargoToml,
    bench: CargoToml,
}

fn graph() -> Graph {
    Graph {
        app: CargoToml::parse(
            "[package]\nname='app'\nversion='1.0.0'\n\
             [dependencies]\nrt='1'\n\
             [dev-dependencies]\nrt={version='1',features=['macros']}\nbench='1'\n",
        )
        .unwrap(),
        rt: CargoToml::parse(
            "[package]\nname='rt'\nversion='1.0.0'\n\
             [features]\nmacros=['dep:rt-macros']\n\
             [dependencies]\nrt-macros={version='1',optional=true}\n",
        )
        .unwrap(),
        macros: CargoToml::parse(
            "[package]\nname='rt-macros'\nversion='1.0.0'\n[lib]\nproc-macro=true\n",
        )
        .unwrap(),
        bench: CargoToml::parse("[package]\nname='bench'\nversion='1.0.0'\n").unwrap(),
    }
}

fn resolve(g: &Graph, resolver: &str) -> mitts::adapter::rust::features::ResolvedFeatures {
    let app_deps = vec!["rt".to_string(), "bench".to_string()];
    let rt_deps = vec!["rt-macros".to_string()];
    let packages = vec![
        ctx("app", &g.app, &app_deps, true),
        ctx("rt", &g.rt, &rt_deps, false),
        ctx("rt-macros", &g.macros, &[], false),
        ctx("bench", &g.bench, &[], false),
    ];
    resolve_package_features_with_resolver(&packages, &RustBuildOptions::default(), Some(resolver))
        .unwrap()
}

#[test]
fn resolver_1_takes_features_from_dev_dependencies_but_does_not_build_them() {
    let r = resolve(&graph(), "1");
    assert!(
        r.target["rt@1.0.0"].contains(&"macros".to_string()),
        "{:?}",
        r.target
    );
    assert!(
        r.active_host_packages.contains("rt-macros@1.0.0"),
        "`macros` builds the proc-macro: {:?}",
        r.active_host_packages
    );
    assert!(
        !r.active_target_packages.contains("bench@1.0.0")
            && !r.active_host_packages.contains("bench@1.0.0"),
        "a dev-only dependency is not built"
    );
}

#[test]
fn resolver_2_keeps_dev_dependency_features_out_of_a_build() {
    let r = resolve(&graph(), "2");
    assert!(
        !r.target["rt@1.0.0"].contains(&"macros".to_string()),
        "{:?}",
        r.target
    );
    assert!(!r.active_host_packages.contains("rt-macros@1.0.0"));
}

/// bevy_image: a marker feature shares its name with an optional dependency hidden by
/// `dep:zstd`; turning the marker on (through `zstd_rust`) must not build the zstd crate.
#[test]
fn a_marker_feature_does_not_turn_on_a_hidden_optional_dependency_of_its_name() {
    let app = CargoToml::parse(
        "[package]\nname='app'\nversion='1.0.0'\nedition='2021'\n\
         [features]\ndefault=['zstd_rust']\nzstd=[]\nzstd_rust=['zstd','dep:ruzstd']\n\
         zstd_c=['zstd','dep:zstd']\n\
         [dependencies]\nzstd={version='1',optional=true}\nruzstd={version='1',optional=true}\n",
    )
    .unwrap();
    let zstd = CargoToml::parse("[package]\nname='zstd'\nversion='1.0.0'\n").unwrap();
    let ruzstd = CargoToml::parse("[package]\nname='ruzstd'\nversion='1.0.0'\n").unwrap();
    let app_deps = vec!["zstd".to_string(), "ruzstd".to_string()];
    let packages = vec![
        ctx("app", &app, &app_deps, true),
        ctx("zstd", &zstd, &[], false),
        ctx("ruzstd", &ruzstd, &[], false),
    ];
    let r = resolve_package_features_with_resolver(&packages, &RustBuildOptions::default(), None)
        .unwrap();
    assert!(r.target["app@1.0.0"].contains(&"zstd".to_string()));
    assert!(r.active_target_packages.contains("ruzstd@1.0.0"));
    assert!(
        !r.active_target_packages.contains("zstd@1.0.0"),
        "{:?}",
        r.active_target_packages
    );
}

/// serde's workspace: the root proc-macro serde_derive asks for `proc-macro2/proc-macro`, and
/// Cargo activates a root proc-macro in the normal context too, so the target proc-macro2 of
/// serde_derive_internals gets `proc-macro` as well. Nothing extra is built for the target.
#[test]
fn a_root_proc_macro_lends_its_dependency_features_to_their_target_copies() {
    let derive = CargoToml::parse(
        "[package]\nname='derive'\nversion='1.0.0'\nedition='2021'\n[lib]\nproc-macro=true\n\
         [dependencies]\npm2={version='1',default-features=false,features=['proc-macro']}\n",
    )
    .unwrap();
    let internals = CargoToml::parse(
        "[package]\nname='internals'\nversion='1.0.0'\nedition='2021'\n\
         [dependencies]\npm2={version='1',default-features=false}\n",
    )
    .unwrap();
    let pm2 = CargoToml::parse(
        "[package]\nname='pm2'\nversion='1.0.0'\n[features]\ndefault=['proc-macro']\nproc-macro=[]\n",
    )
    .unwrap();
    let deps = vec!["pm2".to_string()];
    let both = vec![
        ctx("derive", &derive, &deps, true),
        ctx("internals", &internals, &deps, true),
        ctx("pm2", &pm2, &[], false),
    ];
    let r = resolve_package_features_with_resolver(&both, &RustBuildOptions::default(), Some("2"))
        .unwrap();
    assert!(r.host["pm2@1.0.0"].contains(&"proc-macro".to_string()));
    assert!(
        r.target["pm2@1.0.0"].contains(&"proc-macro".to_string()),
        "{:?}",
        r.target
    );

    let derive_only = vec![
        ctx("derive", &derive, &deps, true),
        ctx("pm2", &pm2, &[], false),
    ];
    let r = resolve_package_features_with_resolver(
        &derive_only,
        &RustBuildOptions::default(),
        Some("2"),
    )
    .unwrap();
    assert!(
        !r.active_target_packages.contains("pm2@1.0.0")
            && !r.active_target_packages.contains("derive@1.0.0"),
        "nothing is built for the target: {:?}",
        r.active_target_packages
    );
}

#[test]
fn all_features_turns_on_implicit_optional_dependency_features() {
    let app = CargoToml::parse(
        "[package]\nname='app'\nversion='1.0.0'\nedition='2021'\n\
         [features]\nfull=[]\nhidden=['dep:quiet']\n\
         [dependencies]\ntracing={version='1',optional=true}\nquiet={version='1',optional=true}\n",
    )
    .unwrap();
    let tracing = CargoToml::parse("[package]\nname='tracing'\nversion='1.0.0'\n").unwrap();
    let quiet = CargoToml::parse("[package]\nname='quiet'\nversion='1.0.0'\n").unwrap();
    let app_deps = vec!["tracing".to_string(), "quiet".to_string()];
    let packages = vec![
        ctx("app", &app, &app_deps, true),
        ctx("tracing", &tracing, &[], false),
        ctx("quiet", &quiet, &[], false),
    ];
    let opt = RustBuildOptions {
        all_features: true,
        ..Default::default()
    };
    let r = resolve_package_features_with_resolver(&packages, &opt, Some("2")).unwrap();
    let feats = &r.target["app@1.0.0"];
    assert!(feats.contains(&"tracing".to_string()), "{feats:?}");
    assert!(
        !feats.contains(&"quiet".to_string()),
        "`dep:` hides the implicit feature"
    );
    assert!(r.active_target_packages.contains("tracing@1.0.0"));
    assert!(
        r.active_target_packages.contains("quiet@1.0.0"),
        "`hidden` turns it on"
    );
}
