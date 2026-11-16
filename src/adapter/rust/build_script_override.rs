/*
 * Mitts Build — build script overrides (`target.<triple>.<links>`)
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A package that declares `links = "foo"` may have its build script replaced by
//! configuration, as in Cargo:
//!
//! ```toml
//! [target.x86_64-unknown-linux-gnu.foo]
//! rustc-link-lib = ["foo"]
//! rustc-link-search = ["/opt/foo/lib"]
//! rustc-flags = "-L /some/path"
//! rustc-cfg = ['key="value"']
//! rustc-env = { KEY = "value" }
//! rustc-cdylib-link-arg = ["..."]
//! metadata_key = "value"   # DEP_FOO_METADATA_KEY for dependents
//! ```
//!
//! The script is then neither compiled nor run; its directives are these values. The
//! triple is the one the package is built for (the host for build-time units). The table
//! is part of the configuration files every action key already hashes, so changing it
//! rebuilds the package and its dependents.

use super::build_script_directives::BuildScriptDirectives;
use super::manifest::CargoToml;
use super::types::PlannedCompilationUnit;
use anyhow::{bail, Result};
use std::fmt::Write as _;
use std::path::Path;

/// Keys of a `target.<triple>` table that are settings of their own, never a `links` name.
const RESERVED: [&str; 5] = ["runner", "linker", "rustflags", "rustdocflags", "ar"];

/// The directives that replace `unit`'s build script, if the configuration overrides it.
pub fn directives_for(
    unit: &PlannedCompilationUnit,
    explicit_target: Option<&str>,
    start_dir: &Path,
) -> Result<Option<BuildScriptDirectives>> {
    let manifest = CargoToml::from_file(&unit.manifest_path).unwrap_or_default();
    let Some(links) = manifest.package.as_ref().and_then(|p| p.links.clone()) else {
        return Ok(None);
    };
    if RESERVED.contains(&links.as_str()) {
        return Ok(None);
    }
    let host = super::compiler_resolve::try_resolve_host_triple()?;
    let triple = if unit.for_host {
        host
    } else {
        super::cargo_config_runner::effective_target(start_dir, explicit_target)?
    };
    let key = format!("target.{triple}.{links}");
    // Every layer, lowest precedence first, merged as Cargo merges configuration: arrays
    // join, tables merge, other values are replaced. No environment variable applies.
    let layers = super::cargo_config_value::tables(start_dir, &key)?;
    if layers.is_empty() {
        return Ok(None);
    }
    let mut table = toml::Table::new();
    for (layer, _, _) in layers {
        merge(&mut table, layer);
    }
    let mut directives = BuildScriptDirectives::parse_stdout(&to_stdout(&table, &key)?);
    directives.links = Some(links);
    directives.overridden = true;
    Ok(Some(directives))
}

fn merge(into: &mut toml::Table, from: toml::Table) {
    for (name, value) in from {
        match (into.get_mut(&name), value) {
            (Some(toml::Value::Array(have)), toml::Value::Array(more)) => have.extend(more),
            (Some(toml::Value::Table(have)), toml::Value::Table(more)) => merge(have, more),
            (_, value) => {
                into.insert(name, value);
            }
        }
    }
}

/// The table as the `cargo:` lines a build script would have printed.
fn to_stdout(table: &toml::Table, key: &str) -> Result<String> {
    let mut out = String::new();
    let strings = |name: &str, value: &toml::Value| -> Result<Vec<String>> {
        match value {
            toml::Value::Array(items) => items
                .iter()
                .map(|item| match item {
                    toml::Value::String(s) => Ok(s.clone()),
                    other => bail!("{key}.{name}: expected strings, found `{other}`"),
                })
                .collect(),
            toml::Value::String(s) => Ok(vec![s.clone()]),
            other => bail!("{key}.{name}: expected an array of strings, found `{other}`"),
        }
    };
    for (name, value) in table {
        match name.as_str() {
            "rustc-link-lib"
            | "rustc-link-search"
            | "rustc-cfg"
            | "rustc-cdylib-link-arg"
            | "rustc-link-arg"
            | "rustc-check-cfg" => {
                for item in strings(name, value)? {
                    let _ = writeln!(out, "cargo:{name}={item}");
                }
            }
            "rustc-flags" => {
                let Some(flags) = value.as_str() else {
                    bail!("{key}.rustc-flags: expected a string");
                };
                let _ = writeln!(out, "cargo:rustc-flags={flags}");
            }
            "rustc-env" => {
                let Some(vars) = value.as_table() else {
                    bail!("{key}.rustc-env: expected a table");
                };
                for (var, val) in vars {
                    let Some(val) = val.as_str() else {
                        bail!("{key}.rustc-env.{var}: expected a string");
                    };
                    let _ = writeln!(out, "cargo:rustc-env={var}={val}");
                }
            }
            "warning" => {}
            other => {
                let text = match value {
                    toml::Value::String(s) => s.clone(),
                    toml::Value::Integer(_) | toml::Value::Boolean(_) | toml::Value::Float(_) => {
                        value.to_string()
                    }
                    _ => bail!("{key}.{other}: expected a string"),
                };
                let _ = writeln!(out, "cargo:metadata={other}={text}");
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_table_becomes_the_lines_a_script_would_print() {
        let table: toml::Table = toml::from_str(
            r#"
            rustc-link-lib = ["foo", "static=bar"]
            rustc-link-search = ["/opt/foo/lib"]
            rustc-cfg = ['has_foo']
            rustc-env = { FOO_VERSION = "1.2" }
            include = "/opt/foo/include"
            "#,
        )
        .unwrap();
        let d = BuildScriptDirectives::parse_stdout(&to_stdout(&table, "target.x.foo").unwrap());
        assert_eq!(d.link_libs, ["foo", "static=bar"]);
        assert_eq!(d.link_searches, ["/opt/foo/lib"]);
        assert_eq!(d.cfgs, ["has_foo"]);
        assert_eq!(d.envs.get("FOO_VERSION").map(String::as_str), Some("1.2"));
        assert_eq!(
            d.metadata.get("include").map(String::as_str),
            Some("/opt/foo/include")
        );
    }

    #[test]
    fn layers_merge_like_cargo_configuration() {
        let mut table: toml::Table = toml::from_str(
            "rustc-link-lib = [\"a\"]\nrustc-env = { X = \"1\" }\ninclude = \"/old\"\n",
        )
        .unwrap();
        merge(
            &mut table,
            toml::from_str(
                "rustc-link-lib = [\"b\"]\nrustc-env = { Y = \"2\" }\ninclude = \"/new\"\n",
            )
            .unwrap(),
        );
        let d = BuildScriptDirectives::parse_stdout(&to_stdout(&table, "k").unwrap());
        assert_eq!(d.link_libs, ["a", "b"]);
        assert_eq!(d.envs.len(), 2);
        assert_eq!(d.metadata.get("include").map(String::as_str), Some("/new"));
    }

    #[test]
    fn a_metadata_change_changes_the_compile_hash_of_an_override() {
        let make = |include: &str| {
            let mut d = BuildScriptDirectives::parse_stdout(&format!(
                "cargo:rustc-link-lib=foo\ncargo:metadata=include={include}\n"
            ));
            d.links = Some("foo".into());
            d.overridden = true;
            d.compute_compile_hash()
        };
        assert_ne!(make("/v1"), make("/v2"));
        assert_eq!(make("/v1"), make("/v1"));
    }

    #[test]
    fn wrong_types_are_errors() {
        let table: toml::Table = toml::from_str("rustc-env = \"X=1\"").unwrap();
        assert!(to_stdout(&table, "target.x.foo").is_err());
    }
}
