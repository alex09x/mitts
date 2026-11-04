/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! An optional dependency whose feature is off is absent from its package's lockfile
//! dependency list. When the lockfile holds several versions of that crate, the dependency
//! is inactive and must be skipped instead of failing with "Ambiguous dependency".

use mitts::adapter::rust::features::PackageFeatureContext;
use mitts::adapter::rust::features_helpers::FeatureGraphIndex;
use mitts::adapter::rust::manifest::{CargoToml, TomlDependency};
use mitts::adapter::RustBuildOptions;

const REGISTRY: &str = "registry+https://github.com/rust-lang/crates.io-index";

fn empty_manifest(name: &str) -> CargoToml {
    CargoToml::parse(&format!(
        "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"
    ))
    .expect("fixture manifest parses")
}

/// A registry package `consumer` declaring `rand` with a requirement that both locked
/// versions satisfy, next to two locked `rand` packages.
fn resolve_rand(consumer_manifest: &str, lock_dependencies: &[String]) -> Option<Option<String>> {
    let consumer_toml = CargoToml::parse(consumer_manifest).expect("consumer manifest parses");
    let rand_old = empty_manifest("rand");
    let rand_new = empty_manifest("rand");
    let packages = [
        PackageFeatureContext {
            name: "consumer",
            version: "0.1.0",
            source: Some(REGISTRY),
            is_workspace: false,
            is_root: false,
            toml: &consumer_toml,
            dependencies: lock_dependencies,
        },
        PackageFeatureContext {
            name: "rand",
            version: "0.8.5",
            source: Some(REGISTRY),
            is_workspace: false,
            is_root: false,
            toml: &rand_old,
            dependencies: &[],
        },
        PackageFeatureContext {
            name: "rand",
            version: "0.9.1",
            source: Some(REGISTRY),
            is_workspace: false,
            is_root: false,
            toml: &rand_new,
            dependencies: &[],
        },
    ];
    let opt = RustBuildOptions::default();
    let index = FeatureGraphIndex::build(&packages, &opt);
    let hint: Option<&TomlDependency> = consumer_toml.dependencies.get("rand");
    index
        .resolve_dep_id_with_hint(0, "rand", hint)
        .ok()
}

const OPTIONAL_RAND: &str = r#"[package]
name = "consumer"
version = "0.1.0"
edition = "2021"

[dependencies]
rand = { version = ">=0.8", optional = true }
other = "1"

[features]
serde = ["rand/serde"]
"#;

fn other_only() -> Vec<String> {
    vec![format!("other 1.0.0 ({REGISTRY})")]
}

#[test]
fn inactive_optional_dependency_is_skipped_when_the_lockfile_omits_it() {
    let resolved = resolve_rand(OPTIONAL_RAND, &other_only());
    assert_eq!(
        resolved,
        Some(None),
        "an optional dependency absent from the lockfile list is inactive, not ambiguous"
    );
}

#[test]
fn required_dependency_missing_from_the_lockfile_is_still_ambiguous() {
    let manifest = OPTIONAL_RAND.replace(", optional = true", "");
    assert_eq!(
        resolve_rand(&manifest, &other_only()),
        None,
        "only optional dependencies may be treated as inactive"
    );
}

#[test]
fn optional_dependency_listed_in_the_lockfile_resolves_to_its_locked_version() {
    let locked = vec![
        format!("other 1.0.0 ({REGISTRY})"),
        format!("rand 0.9.1 ({REGISTRY})"),
    ];
    let resolved = resolve_rand(OPTIONAL_RAND, &locked).expect("active dependency resolves");
    let id = resolved.expect("active dependency has an id");
    assert!(id.contains("rand") && id.contains("0.9.1"), "got {id}");
}

#[test]
fn package_without_lockfile_dependencies_keeps_the_ambiguity_error() {
    assert_eq!(
        resolve_rand(OPTIONAL_RAND, &[]),
        None,
        "without a lockfile list nothing proves the dependency is inactive"
    );
}
