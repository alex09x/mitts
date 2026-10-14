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
}

fn default_true() -> bool {
    true
}

/// Client for crates.io HTTP sparse index protocol (`https://index.crates.io/`).
pub struct SparseIndexClient {
    base_url: String,
    cache_dir: PathBuf,
    agent: ureq::Agent,
}

impl SparseIndexClient {
    /// Creates a new SparseIndexClient caching downloaded index metadata in `~/.cache/mitts/index`.
    pub fn new() -> Result<Self> {
        let base_dir = if let Ok(val) = std::env::var("MITTS_CACHE_DIR") {
            PathBuf::from(val)
        } else if let Ok(home) = std::env::var("HOME") {
            PathBuf::from(home).join(".cache").join("mitts")
        } else {
            PathBuf::from(".mitts").join("cache")
        };

        let cache_dir = base_dir.join("index");
        fs::create_dir_all(&cache_dir)?;

        let agent = ureq::AgentBuilder::new()
            .timeout_connect(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(30))
            .max_idle_connections(64)
            .max_idle_connections_per_host(64)
            .build();

        let base_url = std::env::var("MITTS_SPARSE_INDEX_URL")
            .unwrap_or_else(|_| "https://index.crates.io".to_string());

        Ok(Self {
            base_url,
            cache_dir,
            agent,
        })
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
            .unwrap_or_else(|_| "https://index.crates.io".to_string());
        Ok(Self {
            base_url,
            cache_dir,
            agent,
        })
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

        if local_cache_path.is_file() {
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

        // Fetch from crates.io CDN
        let url = format!("{}/{}", self.base_url, rel_path);
        let resp = match self.agent.get(&url).call() {
            Ok(r) => r,
            Err(ureq::Error::Status(404, _)) => {
                bail!(
                    "Crate '{}' not found in crates.io index ({})",
                    crate_name,
                    url
                );
            }
            Err(e) => {
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
            if let Ok(entry) = serde_json::from_str::<SparseIndexEntry>(trimmed) {
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
