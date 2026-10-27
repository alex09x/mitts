/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Where crates come from, as Cargo configures it: `[registries.<name>]` for alternative
//! registries, `[source.<name>]` with `replace-with` for mirrors and `cargo vendor`
//! directories, tokens from `credentials.toml`, `[registries.<name>] token` and
//! `CARGO_REGISTRIES_<NAME>_TOKEN`. A lockfile keeps the original source id; this module
//! says where its bytes really come from.

use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The source id Cargo writes for crates.io packages.
pub const CRATES_IO_LOCK_SOURCE: &str = "registry+https://github.com/rust-lang/crates.io-index";
/// The sparse index crates.io serves.
pub const CRATES_IO_INDEX_URL: &str = "https://index.crates.io";

#[derive(Debug, Clone, PartialEq, Eq)]
enum SourceDef {
    /// A registry, as a lock-style source id (`sparse+…` or `registry+…`).
    Registry(String),
    /// A git source: the `git+<url>?<ref>` prefix a lock source starts with.
    Git(String),
    Directory(PathBuf),
    LocalRegistry(PathBuf),
}

#[derive(Debug, Clone, Default)]
struct SourceEntry {
    def: Option<SourceDef>,
    replace_with: Option<String>,
}

/// Where a lock source's packages really come from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EffectiveSource {
    /// crates.io with its own index and download host.
    CratesIo,
    /// An HTTP sparse index: a crates.io mirror or an alternative registry.
    Sparse {
        index_url: String,
        token: Option<String>,
    },
    /// A `cargo vendor` directory.
    Directory(PathBuf),
    /// The git repository named by the lock source itself.
    Git,
}

#[derive(Debug, Clone, Default)]
pub struct RegistryConfig {
    /// Registry name → lock-style index source id.
    registries: HashMap<String, String>,
    /// Registry name → token from config or credentials.
    tokens: HashMap<String, String>,
    sources: HashMap<String, SourceEntry>,
}

impl RegistryConfig {
    /// Merges every Cargo config that applies to `start_dir` (lowest precedence first) with
    /// `$CARGO_HOME/credentials.toml`.
    pub fn load(start_dir: &Path) -> Result<Self> {
        let mut cfg = Self::default();
        for path in super::cargo_config::discover_cargo_config_files(start_dir) {
            let text = std::fs::read_to_string(&path)
                .with_context(|| format!("reading {}", path.display()))?;
            let value: toml::Value =
                toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
            // Relative paths in a config file resolve against the parent of its `.cargo`.
            let base = path
                .parent()
                .and_then(Path::parent)
                .map(Path::to_path_buf)
                .unwrap_or_default();
            cfg.merge(&value, &base);
        }
        if let Some(home) = crate::core::source_store::SourceStore::active_cargo_home() {
            for name in ["credentials.toml", "credentials"] {
                let path = home.join(name);
                if let Ok(text) = std::fs::read_to_string(&path) {
                    if let Ok(value) = toml::from_str::<toml::Value>(&text) {
                        cfg.merge_tokens(&value);
                    }
                }
            }
        }
        Ok(cfg)
    }

    fn merge(&mut self, value: &toml::Value, base: &Path) {
        if let Some(regs) = value.get("registries").and_then(|v| v.as_table()) {
            for (name, def) in regs {
                if let Some(index) = def.get("index").and_then(|i| i.as_str()) {
                    self.registries
                        .insert(name.clone(), index_to_lock_source(index));
                }
            }
        }
        self.merge_tokens(value);
        if let Some(sources) = value.get("source").and_then(|v| v.as_table()) {
            for (name, def) in sources {
                let entry = self.sources.entry(name.clone()).or_default();
                if let Some(s) = def.get("replace-with").and_then(|v| v.as_str()) {
                    entry.replace_with = Some(s.to_string());
                }
                if let Some(s) = def.get("registry").and_then(|v| v.as_str()) {
                    entry.def = Some(SourceDef::Registry(index_to_lock_source(s)));
                } else if let Some(s) = def.get("directory").and_then(|v| v.as_str()) {
                    entry.def = Some(SourceDef::Directory(base.join(s)));
                } else if let Some(s) = def.get("local-registry").and_then(|v| v.as_str()) {
                    entry.def = Some(SourceDef::LocalRegistry(base.join(s)));
                } else if let Some(url) = def.get("git").and_then(|v| v.as_str()) {
                    let query = ["branch", "tag", "rev"]
                        .iter()
                        .find_map(|k| {
                            def.get(k)
                                .and_then(|v| v.as_str())
                                .map(|v| format!("?{k}={v}"))
                        })
                        .unwrap_or_default();
                    entry.def = Some(SourceDef::Git(format!("git+{url}{query}")));
                }
            }
        }
    }

    fn merge_tokens(&mut self, value: &toml::Value) {
        if let Some(regs) = value.get("registries").and_then(|v| v.as_table()) {
            for (name, def) in regs {
                if let Some(token) = def.get("token").and_then(|t| t.as_str()) {
                    self.tokens.insert(name.clone(), token.to_string());
                }
            }
        }
    }

    fn env_key(name: &str, suffix: &str) -> String {
        format!(
            "CARGO_REGISTRIES_{}_{suffix}",
            name.to_uppercase().replace('-', "_")
        )
    }

    /// The lock source id of a dependency's `registry = "<name>"` (`None` = crates.io).
    pub fn lock_source_for_registry(&self, name: Option<&str>) -> Result<String> {
        let Some(name) = name else {
            return Ok(CRATES_IO_LOCK_SOURCE.to_string());
        };
        if name == "crates-io" {
            return Ok(CRATES_IO_LOCK_SOURCE.to_string());
        }
        let source = std::env::var(Self::env_key(name, "INDEX"))
            .ok()
            .map(|i| index_to_lock_source(&i))
            .or_else(|| self.registries.get(name).cloned())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "registry `{name}` is not defined: add `[registries.{name}] index = \"sparse+https://…\"` to a Cargo config"
                )
            })?;
        Ok(source)
    }

    fn registry_name_for(&self, lock_source: &str) -> Option<String> {
        self.registries
            .iter()
            .find(|(_, s)| s.as_str() == lock_source)
            .map(|(n, _)| n.clone())
    }

    /// The token for a sparse index, if one is configured anywhere Cargo would look.
    pub fn token_for_index(&self, lock_source: &str) -> Option<String> {
        let name = self.registry_name_for(lock_source)?;
        std::env::var(Self::env_key(&name, "TOKEN"))
            .ok()
            .or_else(|| self.tokens.get(&name).cloned())
    }

    fn source_name_for(&self, lock_source: &str) -> Option<String> {
        if is_crates_io(lock_source) {
            return Some("crates-io".to_string());
        }
        let git_prefix = lock_source.split('#').next().unwrap_or(lock_source);
        self.sources
            .iter()
            .find(|(_, e)| match &e.def {
                Some(SourceDef::Registry(s)) => s == lock_source,
                Some(SourceDef::Git(prefix)) => prefix == git_prefix,
                _ => false,
            })
            .map(|(n, _)| n.clone())
    }

    /// Where the packages of a lock source come from after `[source]` replacement.
    pub fn effective_source(&self, lock_source: &str) -> Result<EffectiveSource> {
        let mut def = None;
        if let Some(mut name) = self.source_name_for(lock_source) {
            let mut hops = 0;
            while let Some(next) = self.sources.get(&name).and_then(|e| e.replace_with.clone()) {
                hops += 1;
                if hops > 8 {
                    bail!("`[source]` replacement loops at `{name}`");
                }
                let entry = self.sources.get(&next).ok_or_else(|| {
                    anyhow::anyhow!(
                        "`[source.{name}] replace-with = \"{next}\"` names an undefined source"
                    )
                })?;
                def = entry.def.clone();
                name = next;
            }
        }
        match def {
            Some(SourceDef::Directory(dir)) => Ok(EffectiveSource::Directory(dir)),
            Some(SourceDef::Registry(s)) => self.sparse_from(&s, lock_source),
            Some(SourceDef::LocalRegistry(p)) => bail!(
                "`local-registry = {:?}` sources are not supported by Mitts; use `directory` (cargo vendor) or a sparse registry",
                p
            ),
            Some(SourceDef::Git(_)) => bail!("replacing a source with a git repository is not supported by Mitts"),
            None => {
                if is_crates_io(lock_source) {
                    Ok(EffectiveSource::CratesIo)
                } else if lock_source.starts_with("git+") {
                    Ok(EffectiveSource::Git)
                } else {
                    self.sparse_from(lock_source, lock_source)
                }
            }
        }
    }

    fn sparse_from(&self, registry_source: &str, lock_source: &str) -> Result<EffectiveSource> {
        if let Some(url) = registry_source.strip_prefix("sparse+") {
            let token = self
                .token_for_index(registry_source)
                .or_else(|| self.token_for_index(lock_source));
            return Ok(EffectiveSource::Sparse {
                index_url: url.trim_end_matches('/').to_string(),
                token,
            });
        }
        if is_crates_io(registry_source) {
            return Ok(EffectiveSource::CratesIo);
        }
        bail!(
            "registry `{}` is a git index; Mitts supports sparse registries only (`sparse+https://…`)",
            registry_source.trim_start_matches("registry+")
        )
    }
}

/// Any of the source ids Cargo has used for crates.io.
pub fn is_crates_io(lock_source: &str) -> bool {
    matches!(
        lock_source,
        CRATES_IO_LOCK_SOURCE
            | "sparse+https://index.crates.io/"
            | "sparse+https://index.crates.io"
    )
}

/// A `[registries] index`/`[source] registry` URL as the lock source id Cargo writes for it.
pub fn index_to_lock_source(index: &str) -> String {
    if index.starts_with("sparse+") {
        index.to_string()
    } else if index == "https://github.com/rust-lang/crates.io-index"
        || index == "https://github.com/rust-lang/crates.io-index.git"
    {
        CRATES_IO_LOCK_SOURCE.to_string()
    } else {
        format!("registry+{index}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(text: &str) -> RegistryConfig {
        let mut c = RegistryConfig::default();
        c.merge(&toml::from_str(text).unwrap(), Path::new("/proj"));
        c
    }

    #[test]
    fn alternative_registry_and_mirror() {
        let c = cfg(
            "[registries]\nmy = { index = \"sparse+https://reg.example/idx/\", token = \"t0k\" }\n[source.crates-io]\nreplace-with = \"mirror\"\n[source.mirror]\nregistry = \"sparse+https://mirror.example/index/\"\n",
        );
        assert_eq!(
            c.lock_source_for_registry(Some("my")).unwrap(),
            "sparse+https://reg.example/idx/"
        );
        assert_eq!(
            c.effective_source("sparse+https://reg.example/idx/")
                .unwrap(),
            EffectiveSource::Sparse {
                index_url: "https://reg.example/idx".into(),
                token: Some("t0k".into())
            }
        );
        assert_eq!(
            c.effective_source(CRATES_IO_LOCK_SOURCE).unwrap(),
            EffectiveSource::Sparse {
                index_url: "https://mirror.example/index".into(),
                token: None
            }
        );
        assert!(c.lock_source_for_registry(Some("nope")).is_err());
    }

    #[test]
    fn vendored_directory_covers_registry_and_git_sources() {
        let c = cfg(
            "[source.crates-io]\nreplace-with = \"vendored-sources\"\n[source.vendored-sources]\ndirectory = \"vendor\"\n[source.\"git+https://github.com/a/b?rev=abc\"]\ngit = \"https://github.com/a/b\"\nrev = \"abc\"\nreplace-with = \"vendored-sources\"\n",
        );
        let dir = EffectiveSource::Directory(PathBuf::from("/proj/vendor"));
        assert_eq!(c.effective_source(CRATES_IO_LOCK_SOURCE).unwrap(), dir);
        assert_eq!(
            c.effective_source("git+https://github.com/a/b?rev=abc#abc123")
                .unwrap(),
            dir
        );
        assert_eq!(
            c.effective_source("git+https://github.com/a/other#1")
                .unwrap(),
            EffectiveSource::Git
        );
    }

    #[test]
    fn git_index_registries_are_rejected_with_advice() {
        let c = cfg("[source.crates-io]\nreplace-with = \"m\"\n[source.m]\nregistry = \"https://rsproxy.example/crates.io-index\"\n");
        let err = c
            .effective_source(CRATES_IO_LOCK_SOURCE)
            .unwrap_err()
            .to_string();
        assert!(err.contains("sparse"), "{err}");
        let plain = RegistryConfig::default();
        assert_eq!(
            plain.effective_source(CRATES_IO_LOCK_SOURCE).unwrap(),
            EffectiveSource::CratesIo
        );
    }
}
