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
    let cainfo = match get("http.cainfo")? {
        Some(value) if !value.as_str()?.trim().is_empty() => Some(value.as_path()?),
        _ => None,
    };
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
pub fn agent_builder() -> Builder {
    builder(true)
}

/// The same for an upload, which has no whole-request deadline by default (a large crate
/// may take longer than 30 s to send).
pub fn upload_agent_builder() -> Builder {
    builder(false)
}

/// Two ureq agents with the same settings, one through the proxy and one direct: a request
/// to a host that `NO_PROXY` names goes direct, as libcurl does for Cargo.
pub struct Builder {
    proxied: Option<ureq::AgentBuilder>,
    direct: ureq::AgentBuilder,
}

impl Builder {
    fn map(self, f: impl Fn(ureq::AgentBuilder) -> ureq::AgentBuilder) -> Self {
        Builder {
            proxied: self.proxied.map(&f),
            direct: f(self.direct),
        }
    }
    pub fn max_idle_connections(self, n: usize) -> Self {
        self.map(|b| b.max_idle_connections(n))
    }
    pub fn max_idle_connections_per_host(self, n: usize) -> Self {
        self.map(|b| b.max_idle_connections_per_host(n))
    }
    pub fn redirects(self, n: u32) -> Self {
        self.map(|b| b.redirects(n))
    }
    pub fn build(self) -> Agent {
        Agent {
            proxied: self.proxied.map(ureq::AgentBuilder::build),
            direct: self.direct.build(),
            no_proxy: NoProxy::from_env(),
        }
    }
}

#[derive(Clone)]
pub struct Agent {
    proxied: Option<ureq::Agent>,
    direct: ureq::Agent,
    no_proxy: NoProxy,
}

impl Agent {
    fn for_url(&self, url: &str) -> &ureq::Agent {
        match &self.proxied {
            Some(proxied) if !self.no_proxy.bypasses(url) => proxied,
            _ => &self.direct,
        }
    }
    pub fn get(&self, url: &str) -> ureq::Request {
        self.for_url(url).get(url)
    }
    pub fn request(&self, method: &str, url: &str) -> ureq::Request {
        self.for_url(url).request(method, url)
    }
    pub fn post(&self, url: &str) -> ureq::Request {
        self.for_url(url).post(url)
    }
    pub fn put(&self, url: &str) -> ureq::Request {
        self.for_url(url).put(url)
    }
}

/// The proxy for this process: `http.proxy`, else the first of `ALL_PROXY`, `HTTPS_PROXY`,
/// `HTTP_PROXY` (either case) that parses.
fn proxy(settings: &HttpSettings) -> Option<ureq::Proxy> {
    if let Some(configured) = settings.proxy.as_deref() {
        // `refresh` checked the URL, so a configured proxy always parses here.
        return ureq::Proxy::new(configured).ok();
    }
    [
        "ALL_PROXY",
        "all_proxy",
        "HTTPS_PROXY",
        "https_proxy",
        "HTTP_PROXY",
        "http_proxy",
    ]
    .iter()
    .filter_map(|name| std::env::var(name).ok())
    .find_map(|value| ureq::Proxy::new(value).ok())
}

fn builder(deadline: bool) -> Builder {
    let settings = settings();
    let base = || {
        let mut builder = ureq::AgentBuilder::new();
        builder = match settings.timeout {
            Some(t) => builder.timeout_connect(t).timeout_read(t),
            None if deadline => builder
                .timeout_connect(Duration::from_secs(10))
                .timeout(Duration::from_secs(30)),
            None => builder,
        };
        if let Some(tls) = settings.tls.clone() {
            builder = builder.tls_config(tls);
        }
        builder
    };
    Builder {
        proxied: proxy(&settings).map(|p| base().proxy(p)),
        direct: base(),
    }
}

/// `NO_PROXY` (else `no_proxy`) as libcurl reads it: comma-separated host names, each also
/// matching its subdomains (a leading `.` or `*.` is the same), IP addresses and CIDR
/// ranges, or `*` for every host. Case does not matter; ports are ignored.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NoProxy {
    all: bool,
    hosts: Vec<String>,
    nets: Vec<(std::net::IpAddr, u8)>,
}

impl NoProxy {
    pub fn from_env() -> Self {
        let value = std::env::var("NO_PROXY")
            .or_else(|_| std::env::var("no_proxy"))
            .unwrap_or_default();
        Self::parse(&value)
    }

    pub fn parse(value: &str) -> Self {
        let mut out = NoProxy::default();
        for entry in value.split(',').map(str::trim).filter(|e| !e.is_empty()) {
            if entry == "*" {
                out.all = true;
                continue;
            }
            if let Some((addr, bits)) = entry.split_once('/') {
                if let (Ok(addr), Ok(bits)) = (addr.parse(), bits.parse::<u8>()) {
                    out.nets.push((addr, bits));
                }
                continue;
            }
            if let Ok(addr) = entry.trim_matches(['[', ']']).parse::<std::net::IpAddr>() {
                let bits = if addr.is_ipv4() { 32 } else { 128 };
                out.nets.push((addr, bits));
                continue;
            }
            let host = entry
                .trim_start_matches("*.")
                .trim_start_matches('.')
                .split(':')
                .next()
                .unwrap_or_default()
                .to_ascii_lowercase();
            if !host.is_empty() {
                out.hosts.push(host);
            }
        }
        out
    }

    /// Whether `url` goes direct.
    pub fn bypasses(&self, url: &str) -> bool {
        if self.all {
            return true;
        }
        let Some(host) = host_of(url) else {
            return false;
        };
        if let Ok(addr) = host.trim_matches(['[', ']']).parse::<std::net::IpAddr>() {
            return self
                .nets
                .iter()
                .any(|(net, bits)| in_net(addr, *net, *bits));
        }
        self.hosts
            .iter()
            .any(|h| host == *h || host.ends_with(&format!(".{h}")))
    }
}

fn host_of(url: &str) -> Option<String> {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = if authority.starts_with('[') {
        authority.split_inclusive(']').next()?
    } else {
        authority.split(':').next()?
    };
    Some(host.to_ascii_lowercase())
}

fn in_net(addr: std::net::IpAddr, net: std::net::IpAddr, bits: u8) -> bool {
    use std::net::IpAddr::{V4, V6};
    let (a, n, width) = match (addr, net) {
        (V4(a), V4(n)) => (u32::from(a) as u128, u32::from(n) as u128, 32u32),
        (V6(a), V6(n)) => (u128::from(a), u128::from(n), 128u32),
        _ => return false,
    };
    let bits = u32::from(bits).min(width);
    if bits == 0 {
        return true;
    }
    let shift = width - bits;
    (a >> shift) == (n >> shift)
}

#[cfg(test)]
pub fn set_for_tests(settings: Option<HttpSettings>) {
    *SETTINGS.write().unwrap_or_else(|e| e.into_inner()) = settings;
}

#[cfg(test)]
mod no_proxy_tests {
    use super::NoProxy;

    #[test]
    fn hosts_subdomains_addresses_and_ranges_go_direct() {
        let rules = NoProxy::parse(
            " Example.com, .internal ,*.corp, 10.0.0.0/8, ::1, 192.168.1.5,host:8080",
        );
        for url in [
            "https://example.com/x",
            "https://index.EXAMPLE.com/config.json",
            "http://registry.internal:8080/",
            "https://a.b.corp",
            "http://10.20.30.40/",
            "http://[::1]:3000/",
            "http://user:pw@192.168.1.5/",
            "http://host/",
        ] {
            assert!(rules.bypasses(url), "{url}");
        }
        for url in [
            "https://notexample.com/",
            "https://index.crates.io/",
            "http://11.0.0.1/",
            "http://192.168.1.6/",
        ] {
            assert!(!rules.bypasses(url), "{url}");
        }
        assert!(NoProxy::parse("*").bypasses("https://index.crates.io/"));
        assert!(!NoProxy::parse("").bypasses("https://index.crates.io/"));
    }
}
