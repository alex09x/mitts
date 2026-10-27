/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SparseIndexEntry {
    pub name: String,
    pub vers: String,
    #[serde(default)]
    pub deps: Vec<SparseIndexDep>,
    pub cksum: String,
    #[serde(default)]
    pub yanked: bool,
    #[serde(default)]
    pub features: HashMap<String, Vec<String>>,
    /// Features using the `dep:` syntax, which crates.io publishes separately.
    #[serde(default)]
    pub features2: HashMap<String, Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SparseIndexDep {
    pub name: String,
    pub req: String,
    #[serde(default)]
    pub features: Vec<String>,
    #[serde(default)]
    pub optional: bool,
    #[serde(default = "default_true")]
    pub default_features: bool,
    pub target: Option<String>,
    pub kind: Option<String>,
    pub package: Option<String>,
    /// Index URL of another registry this dependency comes from (`None`: the same one).
    #[serde(default)]
    pub registry: Option<String>,
}

fn default_true() -> bool {
    true
}

/// Client for crates.io HTTP sparse index protocol (`https://index.crates.io/`).
pub struct SparseIndexClient {
    base_url: String,
    cache_dir: PathBuf,
    agent: ureq::Agent,
    token: Option<String>,
    /// `None` once the index proved to have no readable `config.json`.
    config: std::sync::OnceLock<Option<IndexConfig>>,
}

/// The index's `config.json`: where `.crate` files are downloaded from.
#[derive(Debug, Clone, Deserialize)]
struct IndexConfig {
    dl: String,
    #[serde(default)]
    api: Option<String>,
    #[serde(default, rename = "auth-required")]
    auth_required: bool,
}

impl SparseIndexClient {
    /// Creates a new SparseIndexClient caching downloaded index metadata under the
    /// default cache root (`<cache root>/index`).
    pub fn new() -> Result<Self> {
        Self::for_cache_dir(None)
    }

    /// Client whose index cache lives under `cache_dir` (`<cache_dir>/index`), or under the
    /// default cache root when `None`.
    pub fn for_cache_dir(cache_dir: Option<&Path>) -> Result<Self> {
        Self::with_cache_dir(
            crate::core::cache_root::resolve(cache_dir).join(crate::core::cache_root::INDEX_DIR),
        )
    }

    /// Client for any sparse index: crates.io keeps `<cache root>/index`, every other index
    /// gets `<cache root>/index/<digest of its URL>/`. `token` goes into `Authorization`.
    pub fn for_index(
        cache_dir: Option<&Path>,
        index_url: &str,
        token: Option<String>,
    ) -> Result<Self> {
        let index_url = index_url.trim_end_matches('/').to_string();
        let root =
            crate::core::cache_root::resolve(cache_dir).join(crate::core::cache_root::INDEX_DIR);
        let dir = if index_url == super::registry_config::CRATES_IO_INDEX_URL {
            root
        } else {
            let digest = <sha2::Sha256 as sha2::Digest>::digest(index_url.as_bytes());
            let short: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
            root.join(short)
        };
        let mut client = Self::with_cache_dir(dir)?;
        client.base_url = index_url;
        client.token = token;
        Ok(client)
    }

    /// Custom cache directory for tests or isolated environments.
    pub fn with_cache_dir(cache_dir: PathBuf) -> Result<Self> {
        fs::create_dir_all(&cache_dir)?;
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(30))
            .max_idle_connections(64)
            .max_idle_connections_per_host(64)
            .build();
        let base_url = std::env::var("MITTS_SPARSE_INDEX_URL")
            .unwrap_or_else(|_| super::registry_config::CRATES_IO_INDEX_URL.to_string());
        Ok(Self {
            base_url,
            cache_dir,
            agent,
            token: None,
            config: std::sync::OnceLock::new(),
        })
    }

    pub fn index_url(&self) -> &str {
        &self.base_url
    }

    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    /// How long a cached entry list is trusted online (`MITTS_INDEX_TTL_SECS`, default 15 min).
    fn index_entry_ttl() -> std::time::Duration {
        let secs = std::env::var("MITTS_INDEX_TTL_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(15 * 60);
        std::time::Duration::from_secs(secs)
    }

    fn request(&self, url: &str) -> ureq::Request {
        let req = self.agent.get(url);
        match &self.token {
            Some(t) => req.set("Authorization", t),
            None => req,
        }
    }

    /// The index's `config.json`, fetched once and cached next to the index entries.
    /// `Ok(None)` when the index serves none (reading entries still works; downloading
    /// does not). A 401/403 means a token is required.
    fn config(&self, offline: bool) -> Result<Option<&IndexConfig>> {
        if let Some(c) = self.config.get() {
            return Ok(c.as_ref());
        }
        let path = self.cache_dir.join("config.json");
        let text = match fs::read_to_string(&path) {
            Ok(t) => Some(t),
            Err(_) if offline => None,
            Err(_) => {
                let url = format!("{}/config.json", self.base_url);
                match self.request(&url).call() {
                    Ok(r) => {
                        let t = r.into_string()?;
                        let _ = fs::write(&path, &t);
                        Some(t)
                    }
                    Err(ureq::Error::Status(401 | 403, _)) => bail!(
                        "index {} requires authentication; set CARGO_REGISTRIES_<NAME>_TOKEN or a token in credentials.toml",
                        self.base_url
                    ),
                    Err(_) => None,
                }
            }
        };
        // Something that is not an index config (an entry, an HTML page) counts as none.
        let parsed = text.and_then(|t| serde_json::from_str::<IndexConfig>(&t).ok());
        if parsed.as_ref().is_some_and(|c| c.auth_required) && self.token.is_none() {
            bail!(
                "index {} requires authentication; set CARGO_REGISTRIES_<NAME>_TOKEN or a token in credentials.toml",
                self.base_url
            );
        }
        let _ = parsed.as_ref().and_then(|c| c.api.as_deref());
        Ok(self.config.get_or_init(|| parsed).as_ref())
    }

    /// The URL a `.crate` file is downloaded from, per the index's `dl` template.
    pub fn download_url(
        &self,
        name: &str,
        version: &str,
        cksum: &str,
        offline: bool,
    ) -> Result<String> {
        let dl = self.config(offline)?.map(|c| c.dl.clone()).ok_or_else(|| {
            anyhow::anyhow!(
                "index {} serves no config.json, so its crates cannot be downloaded",
                self.base_url
            )
        })?;
        let markers = [
            "{crate}",
            "{version}",
            "{prefix}",
            "{lowerprefix}",
            "{sha256-checksum}",
        ];
        if !markers.iter().any(|m| dl.contains(m)) {
            return Ok(format!(
                "{}/{name}/{version}/download",
                dl.trim_end_matches('/')
            ));
        }
        let prefix = Self::relative_path(name);
        let prefix = prefix
            .rsplit_once('/')
            .map(|(p, _)| p.to_string())
            .unwrap_or_default();
        Ok(dl
            .replace("{crate}", name)
            .replace("{version}", version)
            .replace("{prefix}", &prefix)
            .replace("{lowerprefix}", &prefix.to_lowercase())
            .replace("{sha256-checksum}", cksum))
    }

    /// Downloads a `.crate` file through this index's download host, with its token.
    pub fn download_crate(&self, name: &str, version: &str, cksum: &str) -> Result<Vec<u8>> {
        let url = self.download_url(name, version, cksum, false)?;
        let resp = self
            .request(&url)
            .call()
            .with_context(|| format!("Failed to download crate from {url}"))?;
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut resp.into_reader(), &mut bytes)
            .with_context(|| format!("Failed to read crate bytes from {url}"))?;
        Ok(bytes)
    }

    /// Computes the relative sparse index path according to crates.io index conventions.
    pub fn relative_path(crate_name: &str) -> String {
        let lower = crate_name.to_lowercase();
        match lower.len() {
            0 => String::new(),
            1 => format!("1/{}", lower),
            2 => format!("2/{}", lower),
            3 => format!("3/{}/{}", &lower[0..1], lower),
            _ => format!("{}/{}/{}", &lower[0..2], &lower[2..4], lower),
        }
    }

    /// Fetches all published versions and metadata for a crate from sparse index.
    pub fn fetch_crate_versions(
        &self,
        crate_name: &str,
        offline: bool,
    ) -> Result<Vec<SparseIndexEntry>> {
        let rel_path = Self::relative_path(crate_name);
        if rel_path.is_empty() {
            bail!("Empty crate name provided");
        }

        let local_cache_path = self.cache_dir.join(&rel_path);

        // A cached entry list serves offline, and online while it is fresh: a lockfile
        // resolved today should see today's versions, as it does with Cargo.
        let fresh = local_cache_path
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age < Self::index_entry_ttl());
        if local_cache_path.is_file() && (offline || fresh) {
            if let Ok(entries) = self.parse_index_file(&local_cache_path) {
                if !entries.is_empty() {
                    return Ok(entries);
                }
            }
        }

        if offline {
            bail!(
                "Crate '{}' not in local index cache and --offline requested",
                crate_name
            );
        }

        // The index's config.json says whether a token is needed; ask before the first entry.
        self.config(offline)?;
        let url = format!("{}/{}", self.base_url, rel_path);
        let resp = match self.request(&url).call() {
            Ok(r) => r,
            Err(ureq::Error::Status(401 | 403, _)) => {
                bail!(
                    "index {} requires authentication; set CARGO_REGISTRIES_<NAME>_TOKEN or a token in credentials.toml",
                    self.base_url
                );
            }
            Err(ureq::Error::Status(404, _)) => {
                bail!("Crate '{}' not found in the index ({})", crate_name, url);
            }
            Err(e) => {
                if let Ok(entries) = self.parse_index_file(&local_cache_path) {
                    if !entries.is_empty() {
                        return Ok(entries);
                    }
                }
                bail!(
                    "Failed to fetch sparse index for '{}' from {}: {}",
                    crate_name,
                    url,
                    e
                );
            }
        };

        let content = resp.into_string().with_context(|| {
            format!(
                "Failed to read sparse index response for '{}' from {}",
                crate_name, url
            )
        })?;

        if let Some(parent) = local_cache_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(&local_cache_path, &content);

        Self::parse_lines(&content)
    }

    /// Parses an on-disk index file containing line-delimited JSON.
    fn parse_index_file(&self, path: &Path) -> Result<Vec<SparseIndexEntry>> {
        let content = fs::read_to_string(path)?;
        Self::parse_lines(&content)
    }

    /// Parses raw line-delimited JSON content.
    pub fn parse_lines(content: &str) -> Result<Vec<SparseIndexEntry>> {
        let mut entries = Vec::new();
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            if let Ok(mut entry) = serde_json::from_str::<SparseIndexEntry>(trimmed) {
                let extra = std::mem::take(&mut entry.features2);
                entry.features.extend(extra);
                entries.push(entry);
            }
        }
        Ok(entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_relative_path() {
        assert_eq!(SparseIndexClient::relative_path("a"), "1/a");
        assert_eq!(SparseIndexClient::relative_path("ab"), "2/ab");
        assert_eq!(SparseIndexClient::relative_path("abc"), "3/a/abc");
        assert_eq!(SparseIndexClient::relative_path("abcd"), "ab/cd/abcd");
        assert_eq!(SparseIndexClient::relative_path("serde"), "se/rd/serde");
        assert_eq!(
            SparseIndexClient::relative_path("serde_json"),
            "se/rd/serde_json"
        );
    }

    #[test]
    fn test_parse_lines() {
        let json_lines = r#"{"name":"foo","vers":"1.0.0","deps":[],"cksum":"deadbeef","features":{},"yanked":false}
{"name":"foo","vers":"1.1.0","deps":[],"cksum":"cafebabe","features":{},"yanked":true}"#;
        let entries = SparseIndexClient::parse_lines(json_lines).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].vers, "1.0.0");
        assert!(!entries[0].yanked);
        assert_eq!(entries[1].vers, "1.1.0");
        assert!(entries[1].yanked);
    }
}
