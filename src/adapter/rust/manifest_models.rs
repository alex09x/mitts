/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

fn deserialize_maybe_bool_or_string<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Helper {
        Str(String),
        #[allow(dead_code)]
        Bool(bool),
        Inherited {
            #[serde(default)]
            #[allow(dead_code)]
            workspace: bool,
        },
    }

    match Option::<Helper>::deserialize(deserializer)? {
        Some(Helper::Str(s)) => Ok(Some(s)),
        _ => Ok(None),
    }
}

/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Deserialize, Default)]
pub struct TomlTest {
    pub name: String,
    pub path: Option<String>,
    #[serde(rename = "required-features")]
    pub required_features: Option<Vec<String>>,
    #[serde(default)]
    pub harness: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct TomlTargetSpec {
    #[serde(default)]
    pub dependencies: HashMap<String, TomlDependency>,
    #[serde(default, rename = "build-dependencies", alias = "build_dependencies")]
    pub build_dependencies: HashMap<String, TomlDependency>,
    #[serde(default, rename = "dev-dependencies", alias = "dev_dependencies")]
    pub dev_dependencies: HashMap<String, TomlDependency>,
}

fn deserialize_maybe_inherited_string<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Helper {
        Str(String),
        Inherited {
            #[serde(default)]
            #[allow(dead_code)]
            workspace: bool,
        },
    }

    match Option::<Helper>::deserialize(deserializer)? {
        Some(Helper::Str(s)) => Ok(Some(s)),
        Some(Helper::Inherited { .. }) => Ok(None),
        None => Ok(None),
    }
}

fn deserialize_maybe_inherited_vec_string<'de, D>(
    deserializer: D,
) -> Result<Option<Vec<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Helper {
        List(Vec<String>),
        Inherited {
            #[serde(default)]
            #[allow(dead_code)]
            workspace: bool,
        },
    }

    match Option::<Helper>::deserialize(deserializer)? {
        Some(Helper::List(s)) => Ok(Some(s)),
        Some(Helper::Inherited { .. }) => Ok(None),
        None => Ok(None),
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct TomlPackage {
    pub name: String,
    #[serde(default, deserialize_with = "deserialize_maybe_inherited_string")]
    pub version: Option<String>,
    #[serde(default, deserialize_with = "deserialize_maybe_inherited_string")]
    pub edition: Option<String>,
    pub build: Option<TomlBuild>,
    pub links: Option<String>,
    #[serde(default)]
    pub autobins: Option<bool>,
    #[serde(default)]
    pub autotests: Option<bool>,
    #[serde(default, deserialize_with = "deserialize_maybe_inherited_string")]
    pub description: Option<String>,
    #[serde(default, deserialize_with = "deserialize_maybe_inherited_string")]
    pub homepage: Option<String>,
    #[serde(default, deserialize_with = "deserialize_maybe_inherited_string")]
    pub repository: Option<String>,
    #[serde(default, deserialize_with = "deserialize_maybe_inherited_vec_string")]
    pub authors: Option<Vec<String>>,
    #[serde(default, deserialize_with = "deserialize_maybe_inherited_string")]
    pub license: Option<String>,
    #[serde(default, deserialize_with = "deserialize_maybe_bool_or_string")]
    pub readme: Option<String>,
    #[serde(
        default,
        rename = "rust-version",
        alias = "rust_version",
        deserialize_with = "deserialize_maybe_inherited_string"
    )]
    pub rust_version: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum TomlBuild {
    Disabled(bool),
    Path(String),
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct TomlWorkspace {
    #[serde(default)]
    pub members: Vec<String>,
    #[serde(default, rename = "default-members")]
    pub default_members: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
    pub package: Option<HashMap<String, toml::Value>>,
    #[serde(default)]
    pub dependencies: HashMap<String, TomlDependency>,
    #[serde(default)]
    pub lints: Option<toml::Value>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct TomlLib {
    pub name: Option<String>,
    pub path: Option<String>,
    #[serde(default, rename = "proc-macro")]
    pub proc_macro: Option<bool>,
    #[serde(default, rename = "crate-type")]
    pub crate_type: Vec<String>,
    #[serde(default)]
    pub harness: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct TomlBin {
    pub name: String,
    pub path: Option<String>,
    #[serde(rename = "required-features")]
    pub required_features: Option<Vec<String>>,
    #[serde(default)]
    pub harness: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum TomlDependency {
    Simple(String),
    Detailed(TomlDetailedDep),
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct TomlDetailedDep {
    pub version: Option<String>,
    pub path: Option<String>,
    pub features: Option<Vec<String>>,
    #[serde(rename = "default-features", alias = "default_features")]
    pub default_features: Option<bool>,
    pub optional: Option<bool>,
    #[serde(rename = "package", alias = "package-name", alias = "package_name")]
    pub package_name: Option<String>,
}

/// Bidirectional mapping between dependency alias and actual package name.
#[derive(Debug, Default, Clone)]
pub struct DependencyAliases {
    pub alias_to_real: HashMap<String, String>,
    pub real_to_aliases: HashMap<String, Vec<(String, Option<String>)>>,
}

/// A discovered compile target within a package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredTarget {
    pub name: String,
    pub kind: String, // "lib", "bin", "proc-macro", "test"
    pub src_path: PathBuf,
    pub edition: Option<String>,
    pub crate_types: Vec<String>,
    pub required_features: Option<Vec<String>>,
    pub harness: bool,
}
