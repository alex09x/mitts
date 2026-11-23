/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A registry's web API (crates.io's `/api/v1`), found the way Cargo finds it: crates.io,
//! or the `api` of a named registry's `config.json`. Tokens come from the registry
//! configuration; a token is only ever sent to that registry's own API host.

use crate::adapter::rust::registry_config::RegistryConfig;
use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::io::Read;
use std::path::Path;

pub struct RegistryApi {
    /// API root, without a trailing slash (`https://crates.io`).
    pub api: String,
    /// Index URL (without `sparse+`).
    pub index: String,
    /// Registry name as the user wrote it (`crates-io` for crates.io).
    pub name: String,
    pub config: RegistryConfig,
    /// `--token` on the command line.
    pub token_override: Option<String>,
}

impl RegistryApi {
    /// The API of `registry` (crates.io when `None`), or of an `--index` URL.
    pub fn open(project_dir: &Path, registry: Option<&str>, index: Option<&str>) -> Result<Self> {
        let config = RegistryConfig::load(project_dir)?;
        let name = registry.unwrap_or("crates-io").to_string();
        let index = match index {
            Some(i) => i.trim_start_matches("sparse+").to_string(),
            None => config.index_url_for(registry)?,
        };
        let crates_io = registry.is_none_or(|r| r == "crates-io") && index_is_crates_io(&index);
        let api = if crates_io {
            std::env::var("MITTS_CRATES_IO_API").unwrap_or_else(|_| "https://crates.io".into())
        } else {
            let config_url = format!("{}/config.json", index.trim_end_matches('/'));
            let doc = get_json(&config_url, None)
                .with_context(|| format!("failed to read the registry configuration at {config_url}"))?;
            match doc["api"].as_str() {
                Some(api) => api.trim_end_matches('/').to_string(),
                None => bail!("the registry `{name}` does not support API commands (no `api` in its config.json)"),
            }
        };
        Ok(Self {
            api,
            index,
            name,
            config,
            token_override: None,
        })
    }

    pub fn token(&self) -> Result<String> {
        if let Some(token) = &self.token_override {
            return Ok(token.clone());
        }
        let name = (self.name != "crates-io").then_some(self.name.as_str());
        self.config
            .token_for_registry(name)?
            .filter(|t| !t.trim().is_empty())
            .with_context(|| {
                format!(
                    "no token found for `{}`, please run `mitts login{}` or set {}",
                    self.name,
                    if self.name == "crates-io" {
                        String::new()
                    } else {
                        format!(" --registry {}", self.name)
                    },
                    if self.name == "crates-io" {
                        "CARGO_REGISTRY_TOKEN".to_string()
                    } else {
                        format!(
                            "CARGO_REGISTRIES_{}_TOKEN",
                            self.name.to_uppercase().replace('-', "_")
                        )
                    }
                )
            })
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}/api/v1{path}", self.api)
    }

    pub fn get(&self, path: &str, token: Option<&str>) -> Result<Value> {
        get_json(&self.url(path), token)
    }

    /// A PUT or DELETE with a JSON body and the registry's token.
    pub fn send(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Value> {
        let token = self.token()?;
        let agent = crate::adapter::rust::http_client::agent_builder().redirects(0).build();
        let request = agent
            .request(method, &self.url(path))
            .set("Authorization", &token)
            .set("Accept", "application/json")
            .set("User-Agent", "mitts");
        let response = match body {
            Some(b) => request.set("Content-Type", "application/json").send_string(&b.to_string()),
            None => request.call(),
        };
        read_response(response)
    }
}

fn index_is_crates_io(index: &str) -> bool {
    let default = std::env::var("MITTS_SPARSE_INDEX_URL")
        .unwrap_or_else(|_| crate::adapter::rust::registry_config::CRATES_IO_INDEX_URL.to_string());
    index.trim_end_matches('/') == default.trim_end_matches('/')
        || index.contains("index.crates.io")
        || index.contains("github.com/rust-lang/crates.io-index")
}

pub fn get_json(url: &str, token: Option<&str>) -> Result<Value> {
    let agent = crate::adapter::rust::http_client::agent_builder().build();
    let mut request = agent
        .get(url)
        .set("Accept", "application/json")
        .set("User-Agent", "mitts");
    if let Some(token) = token {
        request = request.set("Authorization", token);
    }
    read_response(request.call())
}

fn read_response(response: Result<ureq::Response, ureq::Error>) -> Result<Value> {
    let (status, response) = match response {
        Ok(r) => (r.status(), r),
        Err(ureq::Error::Status(code, r)) => (code, r),
        Err(e) => bail!("the request failed: {e}"),
    };
    let mut bytes = Vec::new();
    response.into_reader().take(8 * 1024 * 1024).read_to_end(&mut bytes)?;
    let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    if !(200..300).contains(&status) || value.get("errors").is_some_and(|e| !e.is_null()) {
        let detail = value["errors"]
            .as_array()
            .map(|errors| {
                errors
                    .iter()
                    .filter_map(|e| e["detail"].as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .filter(|d| !d.is_empty())
            .unwrap_or_else(|| format!("status {status}"));
        bail!("the registry responded with an error: {detail}");
    }
    Ok(value)
}

/// The `.crate` archive of `name` `version` from the registry whose sparse index is
/// `index`, checked against `checksum` when one is given.
pub fn download_crate(index: &str, name: &str, version: &str, checksum: Option<&str>) -> Result<Vec<u8>> {
    let doc = get_json(&format!("{}/config.json", index.trim_end_matches('/')), None)?;
    let dl = doc["dl"].as_str().context("the registry has no `dl` URL")?;
    let lower = name.to_lowercase();
    let prefix = match lower.len() {
        1 => "1".to_string(),
        2 => "2".to_string(),
        3 => format!("3/{}", &lower[..1]),
        _ => format!("{}/{}", &lower[..2], &lower[2..4]),
    };
    let url = if ["{crate}", "{version}", "{prefix}", "{lowerprefix}", "{sha256-checksum}"]
        .iter()
        .any(|m| dl.contains(m))
    {
        dl.replace("{crate}", name)
            .replace("{version}", version)
            .replace("{prefix}", &prefix)
            .replace("{lowerprefix}", &prefix)
            .replace("{sha256-checksum}", checksum.unwrap_or_default())
    } else {
        format!("{}/{name}/{version}/download", dl.trim_end_matches('/'))
    };
    let agent = crate::adapter::rust::http_client::agent_builder().build();
    let response = agent
        .get(&url)
        .set("User-Agent", "mitts")
        .call()
        .map_err(|e| anyhow::anyhow!("failed to download {name} v{version}: {e}"))?;
    let mut bytes = Vec::new();
    response.into_reader().take(512 * 1024 * 1024).read_to_end(&mut bytes)?;
    if let Some(expected) = checksum {
        let actual = crate::core::hash::Sha256::digest(&bytes);
        if actual != expected {
            bail!("the checksum of {name} v{version} does not match the registry index");
        }
    }
    Ok(bytes)
}

/// Percent-encodes a query value.
pub fn encode(text: &str) -> String {
    let mut out = String::new();
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}
