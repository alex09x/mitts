/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Issue 143, slint master: two feature-resolution gaps against Cargo.
//!
//! 1. i-slint-compiler has `serde` as a required dependency and as a feature, and its `std`
//!    feature lists `serde/std`. Cargo enables only serde's `std`; Mitts also turned on the
//!    `serde` feature, which pulled in `either/serde` without the `serde` edge in Cargo.lock.
//! 2. i-slint-renderer-skia's `wgpu-30` feature names `dep:wgpu-hal-30`, declared only for
//!    Apple targets, with two wgpu-hal versions locked under renamed keys. On Linux Mitts tried
//!    to resolve it anyway and failed with "Ambiguous dependency 'wgpu-hal'".

use mitts::adapter::rust::features::{resolve_package_features, PackageFeatureContext};
use mitts::adapter::rust::manifest::CargoToml;
use mitts::adapter::rust::types::RustBuildOptions;

fn ctx<'a>(
    name: &'a str,
    version: &'a str,
    toml: &'a CargoToml,
    deps: &'a [String],
    root: bool,
) -> PackageFeatureContext<'a> {
    PackageFeatureContext {
        name,
        version,
        source: None,
        is_workspace: root,
        is_root: root,
        toml,
        dependencies: deps,
    }
}

#[test]
fn a_required_dependency_named_like_a_feature_does_not_turn_that_feature_on() {
    let app = CargoToml::parse(
        "[package]\nname='app'\nversion='0.1.0'\n[dependencies]\ncomp={path='comp'}\n",
    )
    .unwrap();
    let comp = CargoToml::parse(
        "[package]\nname='comp'\nversion='0.1.0'\n\
         [features]\ndefault=['std']\nstd=['serde/std']\nserde=['serde/rc','either/serde']\n\
         [dependencies]\nserde={version='1',default-features=false}\n\
         either={version='1',default-features=false}\n",
    )
    .unwrap();
    let serde =
        CargoToml::parse("[package]\nname='serde'\nversion='1.0.0'\n[features]\nstd=[]\nrc=[]\n")
            .unwrap();
    let either = CargoToml::parse(
        "[package]\nname='either'\nversion='1.0.0'\n\
         [dependencies]\nserde={version='1',optional=true}\n",
    )
    .unwrap();
    let app_deps = vec!["comp".to_string()];
    let comp_deps = vec!["either".to_string(), "serde".to_string()];
    let packages = vec![
        ctx("app", "0.1.0", &app, &app_deps, true),
        ctx("comp", "0.1.0", &comp, &comp_deps, false),
        ctx("serde", "1.0.0", &serde, &[], false),
        ctx("either", "1.0.0", &either, &[], false),
    ];
    let result = resolve_package_features(&packages, &RustBuildOptions::default()).unwrap();
    let comp_feats = &result["comp@0.1.0"];
    assert!(comp_feats.contains(&"std".to_string()), "{comp_feats:?}");
    assert!(
        !comp_feats.contains(&"serde".to_string()),
        "`serde/std` only enables serde's `std`: {comp_feats:?}"
    );
    assert!(result["serde@1.0.0"].contains(&"std".to_string()));
    assert!(!result["serde@1.0.0"].contains(&"rc".to_string()));
    assert!(
        !result
            .get("either@1.0.0")
            .is_some_and(|f| f.contains(&"serde".to_string())),
        "{:?}",
        result.get("either@1.0.0")
    );
}

#[test]
fn an_optional_dependency_still_turns_on_its_same_named_feature() {
    let app = CargoToml::parse(
        "[package]\nname='app'\nversion='0.1.0'\n[features]\ndefault=['serde/std']\n\
         [dependencies]\nserde={version='1',optional=true}\n",
    )
    .unwrap();
    let serde =
        CargoToml::parse("[package]\nname='serde'\nversion='1.0.0'\n[features]\nstd=[]\n").unwrap();
    let app_deps = vec!["serde".to_string()];
    let packages = vec![
        ctx("app", "0.1.0", &app, &app_deps, true),
        ctx("serde", "1.0.0", &serde, &[], false),
    ];
    let result = resolve_package_features(&packages, &RustBuildOptions::default()).unwrap();
    assert!(
        result["app@0.1.0"].contains(&"serde".to_string()),
        "{:?}",
        result["app@0.1.0"]
    );
    assert!(result["serde@1.0.0"].contains(&"std".to_string()));
}

#[test]
fn a_dep_feature_for_a_dependency_of_another_platform_enables_nothing() {
    let app = CargoToml::parse(
        "[package]\nname='app'\nversion='0.1.0'\n\
         [features]\ndefault=['hal-new']\nhal-new=['dep:hal-2']\nhal-old=['dep:hal-1']\n\
         [target.'cfg(target_os = \"no-such-os\")'.dependencies]\n\
         hal-1={package='hal',version='1',optional=true}\n\
         hal-2={package='hal',version='2',optional=true}\n",
    )
    .unwrap();
    let hal1 = CargoToml::parse("[package]\nname='hal'\nversion='1.0.0'\n").unwrap();
    let hal2 = CargoToml::parse("[package]\nname='hal'\nversion='2.0.0'\n").unwrap();
    let app_deps = vec!["hal 1.0.0".to_string(), "hal 2.0.0".to_string()];
    let packages = vec![
        ctx("app", "0.1.0", &app, &app_deps, true),
        ctx("hal", "1.0.0", &hal1, &[], false),
        ctx("hal", "2.0.0", &hal2, &[], false),
    ];
    let result = resolve_package_features(&packages, &RustBuildOptions::default())
        .expect("a dependency of another platform is not resolved, so it is not ambiguous");
    assert!(result["app@0.1.0"].contains(&"hal-new".to_string()));
}

/// A weak feature reaches the build script's required `foo`, not the package's own optional one.
#[test]
fn a_weak_feature_reaches_only_the_side_whose_declaration_is_on() {
    // Resolver 2 (edition 2021) keeps host and target features apart.
    let app = CargoToml::parse(
        "[package]\nname='app'\nversion='0.1.0'\nedition='2021'\n\
         [dependencies]\ncomp={path='comp'}\n",
    )
    .unwrap();
    let comp = CargoToml::parse(
        "[package]\nname='comp'\nversion='0.1.0'\n\
         [features]\ndefault=['x']\nx=['foo?/feat']\n\
         [dependencies]\nfoo={version='1',optional=true}\n\
         [build-dependencies]\nfoo={version='1'}\n",
    )
    .unwrap();
    let foo =
        CargoToml::parse("[package]\nname='foo'\nversion='1.0.0'\n[features]\nfeat=[]\n").unwrap();
    let app_deps = vec!["comp".to_string()];
    let comp_deps = vec!["foo".to_string()];
    let packages = vec![
        ctx("app", "0.1.0", &app, &app_deps, true),
        ctx("comp", "0.1.0", &comp, &comp_deps, false),
        ctx("foo", "1.0.0", &foo, &[], false),
    ];
    let result = resolve_package_features(&packages, &RustBuildOptions::default()).unwrap();
    assert!(
        result.host["foo@1.0.0"].contains(&"feat".to_string()),
        "{:?}",
        result.host
    );
    assert!(
        !result
            .target
            .get("foo@1.0.0")
            .is_some_and(|f| f.contains(&"feat".to_string())),
        "the optional runtime `foo` is off: {:?}",
        result.target
    );
    assert!(!result.active_target_packages.contains("foo@1.0.0"));
}

/// `hal-30/metal` for an Apple-only key must not land on the required `hal` 1 sibling.
#[test]
fn a_feature_of_another_platforms_renamed_dependency_skips_its_same_package_sibling() {
    let app = CargoToml::parse(
        "[package]\nname='app'\nversion='0.1.0'\n\
         [features]\ndefault=['w']\nw=['hal-30/metal']\n\
         [dependencies]\nhal='1'\n\
         [target.'cfg(target_os = \"no-such-os\")'.dependencies]\n\
         hal-30={package='hal',version='30',optional=true}\n",
    )
    .unwrap();
    let hal1 =
        CargoToml::parse("[package]\nname='hal'\nversion='1.0.0'\n[features]\nmetal=[]\n").unwrap();
    let hal30 = CargoToml::parse("[package]\nname='hal'\nversion='30.0.0'\n[features]\nmetal=[]\n")
        .unwrap();
    let app_deps = vec!["hal 1.0.0".to_string(), "hal 30.0.0".to_string()];
    let packages = vec![
        ctx("app", "0.1.0", &app, &app_deps, true),
        ctx("hal", "1.0.0", &hal1, &[], false),
        ctx("hal", "30.0.0", &hal30, &[], false),
    ];
    let result = resolve_package_features(&packages, &RustBuildOptions::default()).unwrap();
    assert!(
        !result
            .get("hal@1.0.0")
            .is_some_and(|f| f.contains(&"metal".to_string())),
        "{:?}",
        result.get("hal@1.0.0")
    );
    assert!(!result["app@0.1.0"].contains(&"hal-30".to_string()));
}

/// `foo/std` for a required `foo` of another platform leaves the same-named feature off.
#[test]
fn a_dep_feature_for_another_platforms_required_dependency_keeps_its_feature_off() {
    let app = CargoToml::parse(
        "[package]\nname='app'\nversion='0.1.0'\n\
         [features]\ndefault=['bar']\nbar=['foo/std']\nfoo=[]\n\
         [target.'cfg(target_os = \"no-such-os\")'.dependencies]\nfoo='1'\n",
    )
    .unwrap();
    let foo =
        CargoToml::parse("[package]\nname='foo'\nversion='1.0.0'\n[features]\nstd=[]\n").unwrap();
    let app_deps = vec!["foo".to_string()];
    let packages = vec![
        ctx("app", "0.1.0", &app, &app_deps, true),
        ctx("foo", "1.0.0", &foo, &[], false),
    ];
    let result = resolve_package_features(&packages, &RustBuildOptions::default()).unwrap();
    let app_feats = &result["app@0.1.0"];
    assert!(app_feats.contains(&"bar".to_string()));
    assert!(!app_feats.contains(&"foo".to_string()), "{app_feats:?}");
}

/// wgpu 30: `wgpu-core` is optional in `[dependencies]` and required for non-wasm targets, so
/// on Linux it is always on and the weak `vulkan = ["wgpu-core?/vulkan"]` reaches it.
#[test]
fn a_weak_feature_reaches_a_dependency_another_declaration_makes_required() {
    let app = CargoToml::parse(
        "[package]\nname='app'\nversion='0.1.0'\n\
         [dependencies]\nwgpu={version='30',default-features=false,features=['vulkan']}\n",
    )
    .unwrap();
    let wgpu = CargoToml::parse(
        "[package]\nname='wgpu'\nversion='30.0.1'\n\
         [features]\nvulkan=['wgpu-core?/vulkan']\n\
         [dependencies]\nwgpu-core={version='30',optional=true}\n\
         [target.'cfg(not(target_arch = \"no-such-arch\"))'.dependencies]\n\
         wgpu-core={version='30',features=['wgsl']}\n",
    )
    .unwrap();
    let core = CargoToml::parse(
        "[package]\nname='wgpu-core'\nversion='30.0.1'\n[features]\nvulkan=[]\nwgsl=[]\n",
    )
    .unwrap();
    let app_deps = vec!["wgpu".to_string()];
    let wgpu_deps = vec!["wgpu-core".to_string()];
    let packages = vec![
        ctx("app", "0.1.0", &app, &app_deps, true),
        ctx("wgpu", "30.0.1", &wgpu, &wgpu_deps, false),
        ctx("wgpu-core", "30.0.1", &core, &[], false),
    ];
    let result = resolve_package_features(&packages, &RustBuildOptions::default()).unwrap();
    let core_feats = &result["wgpu-core@30.0.1"];
    assert!(
        core_feats.contains(&"vulkan".to_string()),
        "the weak feature reaches the required wgpu-core: {core_feats:?}"
    );
    assert!(core_feats.contains(&"wgsl".to_string()));
}
