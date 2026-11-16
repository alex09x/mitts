/*
 * Mitts Build — HTTP client settings from Cargo configuration
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Every registry request (sparse index, downloads, publish) goes through an agent built
//! here, so Cargo's HTTP settings apply to all of them:
//!
//! - `http.proxy` / `CARGO_HTTP_PROXY`; without it the usual `HTTPS_PROXY`, `HTTP_PROXY`,
//!   `ALL_PROXY` variables (either case), as libcurl reads them for Cargo;
//! - `http.cainfo` / `CARGO_HTTP_CAINFO`: a PEM bundle that replaces the built-in roots;
//! - `http.timeout` / `CARGO_HTTP_TIMEOUT`: seconds to connect and between reads (default:
//!   10 s to connect, 30 s per request).

use super::cargo_config_value;
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;

#[derive(Clone, Default)]
pub struct HttpSettings {
    pub proxy: Option<String>,
    pub cainfo: Option<PathBuf>,
    pub timeout: Option<Duration>,
    tls: Option<Arc<rustls::ClientConfig>>,
}

static SETTINGS: RwLock<Option<HttpSettings>> = RwLock::new(None);

/// Rereads the settings for a command started in `start_dir`.
pub fn refresh(start_dir: &Path) -> Result<()> {
    let settings = read(start_dir)?;
    *SETTINGS.write().unwrap_or_else(|e| e.into_inner()) = Some(settings);
    Ok(())
}

fn read(start_dir: &Path) -> Result<HttpSettings> {
    let get = |key: &str| cargo_config_value::get(start_dir, key);
    let proxy = get("http.proxy")?
        .map(|v| v.as_str().map(|s| s.trim().to_string()))
        .transpose()?
        .filter(|s| !s.is_empty());
    if let Some(proxy) = &proxy {
        if let Err(error) = ureq::Proxy::new(proxy) {
            bail!("http.proxy `{proxy}` is not a usable proxy URL: {error}");
        }
    }
    let cainfo = get("http.cainfo")?.map(|v| v.as_path()).transpose()?;
    let timeout = match get("http.timeout")? {
        Some(value) => {
            let secs = value.as_i64()?;
            if secs <= 0 {
                bail!(
                    "{}: http.timeout must be a positive number of seconds",
                    value.origin
                );
            }
            Some(Duration::from_secs(secs as u64))
        }
        None => None,
    };
    let tls = cainfo.as_deref().map(tls_config).transpose()?;
    Ok(HttpSettings {
        proxy,
        cainfo,
        timeout,
        tls,
    })
}

/// A rustls configuration that trusts exactly the certificates in `bundle`.
fn tls_config(bundle: &Path) -> Result<Arc<rustls::ClientConfig>> {
    use rustls::pki_types::{pem::PemObject, CertificateDer};
    let mut roots = rustls::RootCertStore::empty();
    let certs = CertificateDer::pem_file_iter(bundle)
        .with_context(|| format!("http.cainfo: cannot read {}", bundle.display()))?;
    for cert in certs {
        let cert =
            cert.with_context(|| format!("http.cainfo: bad certificate in {}", bundle.display()))?;
        roots.add(cert).with_context(|| {
            format!("http.cainfo: unusable certificate in {}", bundle.display())
        })?;
    }
    if roots.is_empty() {
        bail!("http.cainfo: no certificates in {}", bundle.display());
    }
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .context("http.cainfo: TLS setup failed")?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(Arc::new(config))
}

pub fn settings() -> HttpSettings {
    SETTINGS
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .unwrap_or_default()
}

/// An agent builder with Cargo's HTTP settings; callers add their own pool sizes.
pub fn agent_builder() -> ureq::AgentBuilder {
    builder(true)
}

/// The same for an upload, which has no whole-request deadline by default (a large crate
/// may take longer than 30 s to send).
pub fn upload_agent_builder() -> ureq::AgentBuilder {
    builder(false)
}

fn builder(deadline: bool) -> ureq::AgentBuilder {
    let settings = settings();
    let mut builder = ureq::AgentBuilder::new();
    builder = match settings.timeout {
        Some(t) => builder.timeout_connect(t).timeout_read(t),
        None if deadline => builder
            .timeout_connect(Duration::from_secs(10))
            .timeout(Duration::from_secs(30)),
        None => builder,
    };
    // `refresh` checked the URL, so a configured proxy always parses here.
    builder = match settings.proxy.as_deref().map(ureq::Proxy::new) {
        Some(Ok(proxy)) => builder.proxy(proxy),
        _ => builder.try_proxy_from_env(true),
    };
    if let Some(tls) = settings.tls {
        builder = builder.tls_config(tls);
    }
    builder
}

#[cfg(test)]
pub fn set_for_tests(settings: Option<HttpSettings>) {
    *SETTINGS.write().unwrap_or_else(|e| e.into_inner()) = settings;
}
