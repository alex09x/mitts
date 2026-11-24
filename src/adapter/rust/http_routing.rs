/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Which way each request goes, as libcurl decides it for Cargo: through the proxy for its
//! scheme (`http.proxy`, else `https_proxy`/`HTTPS_PROXY` for https and `http_proxy` for
//! http, else `all_proxy`/`ALL_PROXY`), or direct when `no_proxy`/`NO_PROXY` names the host.
//! Redirects are followed here, one hop at a time, so every hop is routed for its own URL.

use std::net::IpAddr;

/// ureq agents with the same settings: direct, and one per scheme with a proxy.
pub struct Builder {
    direct: ureq::AgentBuilder,
    http: Option<ureq::AgentBuilder>,
    https: Option<ureq::AgentBuilder>,
    redirects: u32,
}

impl Builder {
    /// `base` makes an agent builder with the timeouts and TLS settings; `configured` is
    /// `http.proxy`, which applies to both schemes.
    pub fn new(base: impl Fn() -> ureq::AgentBuilder, configured: Option<&str>) -> Builder {
        let with = |proxy: Option<ureq::Proxy>| proxy.map(|p| base().proxy(p));
        let (http, https) = match configured.and_then(|c| ureq::Proxy::new(c).ok()) {
            Some(p) => (Some(p.clone()), Some(p)),
            None => (env_proxy("http"), env_proxy("https")),
        };
        Builder {
            direct: base(),
            http: with(http),
            https: with(https),
            redirects: 5,
        }
    }

    fn map(self, f: impl Fn(ureq::AgentBuilder) -> ureq::AgentBuilder) -> Self {
        Builder {
            direct: f(self.direct),
            http: self.http.map(&f),
            https: self.https.map(&f),
            redirects: self.redirects,
        }
    }
    pub fn max_idle_connections(self, n: usize) -> Self {
        self.map(|b| b.max_idle_connections(n))
    }
    pub fn max_idle_connections_per_host(self, n: usize) -> Self {
        self.map(|b| b.max_idle_connections_per_host(n))
    }
    /// How many redirects a request follows (0: none).
    pub fn redirects(mut self, n: u32) -> Self {
        self.redirects = n;
        self
    }
    pub fn build(self) -> Agent {
        // ureq itself follows none: `Request` does, routing each hop.
        let build = |b: ureq::AgentBuilder| b.redirects(0).build();
        Agent {
            direct: build(self.direct),
            http: self.http.map(build),
            https: self.https.map(build),
            no_proxy: NoProxy::from_env(),
            redirects: self.redirects,
        }
    }
}

/// libcurl's environment proxy for `scheme`: the lowercase variable first; plain http reads
/// only `http_proxy` (curl ignores `HTTP_PROXY`); then `all_proxy`, `ALL_PROXY`.
fn env_proxy(scheme: &str) -> Option<ureq::Proxy> {
    let names: &[&str] = if scheme == "https" {
        &["https_proxy", "HTTPS_PROXY", "all_proxy", "ALL_PROXY"]
    } else {
        &["http_proxy", "all_proxy", "ALL_PROXY"]
    };
    names
        .iter()
        .filter_map(|n| std::env::var(n).ok().filter(|v| !v.trim().is_empty()))
        .find_map(|v| ureq::Proxy::new(v).ok())
}

#[derive(Clone)]
pub struct Agent {
    direct: ureq::Agent,
    http: Option<ureq::Agent>,
    https: Option<ureq::Agent>,
    no_proxy: NoProxy,
    redirects: u32,
}

impl Agent {
    fn for_url(&self, url: &str) -> &ureq::Agent {
        if self.no_proxy.bypasses(url) {
            return &self.direct;
        }
        // Schemes are case-insensitive (`HTTPS://` is https).
        let https = url
            .get(..8)
            .is_some_and(|s| s.eq_ignore_ascii_case("https://"));
        let proxied = if https { &self.https } else { &self.http };
        proxied.as_ref().unwrap_or(&self.direct)
    }
    pub fn request(&self, method: &str, url: &str) -> Request {
        Request {
            agent: self.clone(),
            method: method.to_string(),
            url: url.to_string(),
            headers: Vec::new(),
        }
    }
    pub fn get(&self, url: &str) -> Request {
        self.request("GET", url)
    }
    pub fn post(&self, url: &str) -> Request {
        self.request("POST", url)
    }
    pub fn put(&self, url: &str) -> Request {
        self.request("PUT", url)
    }
}

/// A request that follows redirects itself (up to the agent's limit), choosing the proxy
/// or the direct route for every hop. As ureq does by default, `Authorization` is not sent
/// after a redirect, and 301/302/303 turn a request with a body into a GET without one.
#[derive(Clone)]
pub struct Request {
    agent: Agent,
    method: String,
    url: String,
    headers: Vec<(String, String)>,
}

// The same `Result<Response, ureq::Error>` as ureq's own `Request`, which callers match on
// (net_retry tells transient errors apart by it); boxing would only move the size.
#[allow(clippy::result_large_err)]
impl Request {
    pub fn set(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }
    pub fn call(self) -> Result<ureq::Response, ureq::Error> {
        self.send(None)
    }
    pub fn send_string(self, body: &str) -> Result<ureq::Response, ureq::Error> {
        self.send(Some(body.as_bytes()))
    }
    pub fn send_bytes(self, body: &[u8]) -> Result<ureq::Response, ureq::Error> {
        self.send(Some(body))
    }

    fn send(self, body: Option<&[u8]>) -> Result<ureq::Response, ureq::Error> {
        let mut method = self.method;
        let mut url = self.url;
        let mut headers = self.headers;
        let mut body = body;
        let mut hops = 0;
        loop {
            let mut request = self.agent.for_url(&url).request(&method, &url);
            for (name, value) in &headers {
                request = request.set(name, value);
            }
            let response = match body {
                Some(bytes) => request.send_bytes(bytes)?,
                None => request.call()?,
            };
            let status = response.status();
            let location = response.header("location").map(str::to_string);
            let (true, Some(location)) = ((300..400).contains(&status), location) else {
                return Ok(response);
            };
            if !matches!(status, 301 | 302 | 303 | 307 | 308) || self.agent.redirects == 0 {
                // Not a redirect to follow, or following is off: the 3xx is the answer.
                return Ok(response);
            }
            if hops >= self.agent.redirects {
                // Too many redirects: an error, never the 3xx body read as the payload.
                return Err(ureq::Error::Status(status, response));
            }
            hops += 1;
            url = join(&url, &location);
            headers.retain(|(n, _)| !n.eq_ignore_ascii_case("authorization"));
            if matches!(status, 301..=303) && method != "GET" && method != "HEAD" {
                method = "GET".to_string();
                body = None;
                headers.retain(|(n, _)| !n.eq_ignore_ascii_case("content-type"));
            }
        }
    }
}

/// `location` resolved against `base` as RFC 3986 section 5 does it: an absolute or
/// network-path reference, an absolute path, a query or fragment alone (same path), or a
/// relative path merged with the base directory, with `.` and `..` segments removed.
pub fn join(base: &str, location: &str) -> String {
    let has_scheme = location.split_once(':').is_some_and(|(s, _)| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
    }) && location.contains("://");
    if has_scheme {
        return normalize(location);
    }
    let (scheme, rest) = base.split_once("://").unwrap_or(("https", base));
    if let Some(net) = location.strip_prefix("//") {
        return normalize(&format!("{scheme}://{net}"));
    }
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let origin = format!("{scheme}://{}", &rest[..authority_end]);
    let after = &rest[authority_end..];
    let path = after.split(['?', '#']).next().unwrap_or("");
    let path_and_query = after.split('#').next().unwrap_or("");
    if location.is_empty() {
        return format!("{origin}{path_and_query}");
    }
    if location.starts_with('#') {
        return format!("{origin}{path_and_query}{location}");
    }
    if location.starts_with('?') {
        return format!("{origin}{path}{location}");
    }
    let (loc_path, loc_tail) = match location.find(['?', '#']) {
        Some(i) => location.split_at(i),
        None => (location, ""),
    };
    let merged = if loc_path.starts_with('/') {
        loc_path.to_string()
    } else {
        let dir = path.rfind('/').map_or("/", |i| &path[..=i]);
        format!("{dir}{loc_path}")
    };
    format!("{origin}{}{loc_tail}", remove_dot_segments(&merged))
}

/// An absolute URL with the dot segments of its path removed, so it is a sound base for
/// the next hop's relative reference.
fn normalize(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_string();
    };
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, after) = rest.split_at(authority_end);
    let path_end = after.find(['?', '#']).unwrap_or(after.len());
    let (path, tail) = after.split_at(path_end);
    if path.is_empty() {
        return url.to_string();
    }
    format!("{scheme}://{authority}{}{tail}", remove_dot_segments(path))
}

fn remove_dot_segments(path: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    let segments: Vec<&str> = path.split('/').collect();
    for (i, segment) in segments.iter().enumerate() {
        let last = i + 1 == segments.len();
        match *segment {
            "." => {
                if last {
                    out.push("");
                }
            }
            ".." => {
                if out.len() > 1 {
                    out.pop();
                }
                if last {
                    out.push("");
                }
            }
            s => out.push(s),
        }
    }
    let joined = out.join("/");
    if joined.starts_with('/') {
        joined
    } else {
        format!("/{joined}")
    }
}

/// `no_proxy` (else `NO_PROXY`), as libcurl reads it: comma-separated host names, each also
/// matching its subdomains (a leading `.` or `*.` is the same), IP addresses and CIDR
/// ranges, or `*` for every host. Case does not matter; ports are ignored.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NoProxy {
    all: bool,
    hosts: Vec<String>,
    nets: Vec<(IpAddr, u8)>,
}

/// The host of an entry or authority without its port: `[::1]:80` -> `::1`, `h:80` -> `h`.
fn strip_port(text: &str) -> &str {
    if let Some(rest) = text.strip_prefix('[') {
        return rest.split(']').next().unwrap_or(rest);
    }
    match text.matches(':').count() {
        1 => text.split(':').next().unwrap_or(text),
        _ => text,
    }
}

impl NoProxy {
    pub fn from_env() -> Self {
        let value = ["no_proxy", "NO_PROXY"]
            .iter()
            .filter_map(|n| std::env::var(n).ok())
            .find(|v| !v.trim().is_empty())
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
                let addr = addr.trim_matches(['[', ']']);
                if let (Ok(addr), Ok(bits)) = (addr.parse(), bits.parse::<u8>()) {
                    out.nets.push((addr, bits));
                }
                continue;
            }
            let host = strip_port(entry);
            if let Ok(addr) = host.parse::<IpAddr>() {
                out.nets.push((addr, if addr.is_ipv4() { 32 } else { 128 }));
                continue;
            }
            let host = host
                .trim_start_matches("*.")
                .trim_start_matches('.')
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
        if let Ok(addr) = host.parse::<IpAddr>() {
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
        strip_port(authority)
    } else {
        authority.split(':').next()?
    };
    Some(host.to_ascii_lowercase())
}

fn in_net(addr: IpAddr, net: IpAddr, bits: u8) -> bool {
    let (a, n, width) = match (addr, net) {
        (IpAddr::V4(a), IpAddr::V4(n)) => (u32::from(a) as u128, u32::from(n) as u128, 32u32),
        (IpAddr::V6(a), IpAddr::V6(n)) => (u128::from(a), u128::from(n), 128u32),
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
#[path = "http_routing_tests.rs"]
mod tests;
