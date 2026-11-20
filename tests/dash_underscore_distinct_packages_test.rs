/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! zed has the git package `livekit-api` (a dependency of the git `livekit`) and its own
//! member `livekit_api`. Mitts folds `-` and `_` to match a manifest's spelling with the
//! lockfile's, saw both, and failed: "Ambiguous dependency specification 'livekit-api' in
//! Cargo.lock (2 matching packages found)". For Cargo they are different packages; the one
//! spelled exactly as the dependency is it.

use mitts::adapter::rust::features::{
    resolve_package_features_with_resolver, PackageFeatureContext,
};
use mitts::adapter::rust::manifest::CargoToml;
use mitts::adapter::rust::types::RustBuildOptions;

const GIT: &str = "git+https://github.com/zed-industries/livekit-rust-sdks?rev=fbda05e#fbda05e";

#[test]
fn a_dash_and_an_underscore_package_are_not_the_same_dependency() {
    let livekit = CargoToml::parse(
        "[package]\nname='livekit'\nversion='0.7.32'\nedition='2021'\n\
         [features]\ndefault=['livekit-api/rustls']\n\
         [dependencies]\nlivekit-api='0.4'\n",
    )
    .unwrap();
    let api_git = CargoToml::parse(
        "[package]\nname='livekit-api'\nversion='0.4.14'\nedition='2021'\n\
         [features]\nrustls=[]\n",
    )
    .unwrap();
    let api_member = CargoToml::parse(
        "[package]\nname='livekit_api'\nversion='0.1.0'\nedition='2021'\n\
         [features]\ntest-support=[]\n",
    )
    .unwrap();
    let app = CargoToml::parse(
        "[package]\nname='app'\nversion='0.1.0'\nedition='2021'\n\
         [dependencies]\nlivekit='0.7'\nlivekit_api={path='../livekit_api'}\n",
    )
    .unwrap();
    let app_deps = vec![format!("livekit 0.7.32 ({GIT})"), "livekit_api".to_string()];
    let livekit_deps = vec![format!("livekit-api 0.4.14 ({GIT})")];
    let packages = vec![
        PackageFeatureContext {
            name: "app",
            version: "0.1.0",
            source: None,
            is_workspace: true,
            is_root: true,
            toml: &app,
            dependencies: &app_deps,
        },
        PackageFeatureContext {
            name: "livekit_api",
            version: "0.1.0",
            source: None,
            is_workspace: true,
            is_root: false,
            toml: &api_member,
            dependencies: &[],
        },
        PackageFeatureContext {
            name: "livekit",
            version: "0.7.32",
            source: Some(GIT),
            is_workspace: false,
            is_root: false,
            toml: &livekit,
            dependencies: &livekit_deps,
        },
        PackageFeatureContext {
            name: "livekit-api",
            version: "0.4.14",
            source: Some(GIT),
            is_workspace: false,
            is_root: false,
            toml: &api_git,
            dependencies: &[],
        },
    ];
    let r = resolve_package_features_with_resolver(&packages, &RustBuildOptions::default(), None)
        .expect("not ambiguous");
    let git_api = format!("livekit-api@0.4.14 ({GIT})");
    assert!(
        r.target
            .get(&git_api)
            .is_some_and(|f| f.contains(&"rustls".to_string())),
        "livekit's feature reaches the git livekit-api: {:?}",
        r.target
    );
    assert!(
        !r.target
            .get("livekit_api@0.1.0")
            .is_some_and(|f| f.contains(&"rustls".to_string())),
        "the member livekit_api gets nothing of it: {:?}",
        r.target
    );
}
