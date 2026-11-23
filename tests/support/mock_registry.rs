/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A sparse registry with a crates.io-like web API on 127.0.0.1, for tests of commands
//! that talk to a registry. Requests are recorded so a test can check what was sent.

#![allow(dead_code)]

use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

/// One published version.
pub struct Published {
    pub name: &'static str,
    pub vers: &'static str,
    /// Index dependency records (see [`dep`]).
    pub deps: Vec<String>,
    pub features: &'static str,
    pub yanked: bool,
    /// Extra files in the crate archive: (path inside the package, contents).
    pub files: Vec<(&'static str, String)>,
    /// `[package]` lines added to the archive's Cargo.toml.
    pub manifest_extra: &'static str,
}

pub fn version(name: &'static str, vers: &'static str) -> Published {
    Published {
        name,
        vers,
        deps: Vec::new(),
        features: "{}",
        yanked: false,
        files: Vec::new(),
        manifest_extra: "",
    }
}

/// An index dependency record of kind `normal`.
pub fn dep(name: &str, req: &str) -> String {
    format!(
        "{{\"name\":\"{name}\",\"req\":\"{req}\",\"features\":[],\"optional\":false,\"default_features\":true,\"target\":null,\"kind\":\"normal\"}}"
    )
}

fn index_path(name: &str) -> String {
    match name.len() {
        1 => format!("1/{name}"),
        2 => format!("2/{name}"),
        3 => format!("3/{}/{name}", &name[..1]),
        _ => format!("{}/{}/{name}", &name[..2], &name[2..4]),
    }
}

/// The `.crate` archive of a published version and its sha256.
pub fn archive(p: &Published) -> (Vec<u8>, String) {
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    {
        let mut tar = tar::Builder::new(&mut gz);
        let deps: String = p
            .deps
            .iter()
            .filter_map(|d| {
                let v: serde_json::Value = serde_json::from_str(d).ok()?;
                Some(format!(
                    "{} = \"{}\"\n",
                    v["name"].as_str()?,
                    v["req"].as_str()?
                ))
            })
            .collect();
        let mut files: Vec<(String, String)> = vec![
            (
                "Cargo.toml".to_string(),
                format!(
                    "[package]\nname = \"{}\"\nversion = \"{}\"\nedition = \"2021\"\n{}\n[dependencies]\n{deps}",
                    p.name, p.vers, p.manifest_extra
                ),
            ),
        ];
        if !p.files.iter().any(|(path, _)| *path == "src/lib.rs" || *path == "src/main.rs") {
            files.push(("src/lib.rs".to_string(), format!("pub fn hello() -> &'static str {{ \"{}\" }}\n", p.name)));
        }
        for (path, data) in &p.files {
            files.push((path.to_string(), data.clone()));
        }
        for (path, data) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append_data(
                &mut header,
                format!("{}-{}/{path}", p.name, p.vers),
                data.as_bytes(),
            )
            .unwrap();
        }
        tar.finish().unwrap();
    }
    let bytes = gz.finish().unwrap();
    let sum = format!("{:x}", Sha256::digest(&bytes));
    (bytes, sum)
}

/// A request the registry received: method, path, headers (lower-case names), body.
#[derive(Clone, Debug)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

pub struct Registry {
    pub url: String,
    pub requests: Arc<Mutex<Vec<Request>>>,
}

impl Registry {
    pub fn index_url(&self) -> String {
        format!("sparse+{}/index/", self.url)
    }

    /// `.cargo/config.toml` text that replaces crates.io with this registry.
    pub fn source_replacement(&self) -> String {
        format!(
            "[source.crates-io]\nreplace-with = \"mock\"\n[source.mock]\nregistry = \"{}\"\n",
            self.index_url()
        )
    }

    pub fn requests(&self) -> Vec<Request> {
        self.requests.lock().unwrap().clone()
    }
}

/// Serves `published` as a sparse index plus downloads and these API routes:
/// `GET /api/v1/crates?q=` (search), `GET|PUT|DELETE /api/v1/crates/NAME/owners`,
/// `DELETE /api/v1/crates/NAME/VERS/yank`, `PUT /api/v1/crates/NAME/VERS/unyank`.
pub fn serve(published: Vec<Published>) -> Registry {
    let mut routes: HashMap<String, Vec<u8>> = HashMap::new();
    let mut lines: HashMap<&str, String> = HashMap::new();
    let mut search: Vec<serde_json::Value> = Vec::new();
    for p in &published {
        let (bytes, sum) = archive(p);
        routes.insert(format!("/dl/{}/{}/download", p.name, p.vers), bytes);
        lines.entry(p.name).or_default().push_str(&format!(
            "{{\"name\":\"{}\",\"vers\":\"{}\",\"deps\":[{}],\"cksum\":\"{sum}\",\"features\":{},\"yanked\":{}}}\n",
            p.name,
            p.vers,
            p.deps.join(","),
            p.features,
            p.yanked
        ));
        search.retain(|c| c["name"] != p.name);
        search.push(serde_json::json!({
            "name": p.name,
            "max_version": p.vers,
            "max_stable_version": p.vers,
            "description": format!("The {} crate", p.name),
        }));
    }
    for (name, text) in lines {
        routes.insert(format!("/index/{}", index_path(name)), text.into_bytes());
    }
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    routes.insert(
        "/index/config.json".to_string(),
        format!("{{\"dl\":\"{base}/dl\",\"api\":\"{base}\",\"auth-required\":false}}").into_bytes(),
    );
    let requests: Arc<Mutex<Vec<Request>>> = Arc::new(Mutex::new(Vec::new()));
    let recorded = requests.clone();
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let Some(request) = read_request(&mut stream) else {
                continue;
            };
            recorded.lock().unwrap().push(request.clone());
            let path_only = request.path.split('?').next().unwrap_or("").to_string();
            let (status, body) = if request.method == "GET" && path_only == "/api/v1/crates" {
                let q = request
                    .path
                    .split_once("q=")
                    .map(|(_, q)| q.split('&').next().unwrap_or("").to_string())
                    .unwrap_or_default();
                let found: Vec<&serde_json::Value> = search
                    .iter()
                    .filter(|c| c["name"].as_str().is_some_and(|n| n.contains(&q)))
                    .collect();
                (
                    "200 OK",
                    serde_json::json!({"crates": found, "meta": {"total": found.len()}})
                        .to_string()
                        .into_bytes(),
                )
            } else if path_only.starts_with("/api/v1/crates/") && path_only.ends_with("/owners") {
                let body = if request.method == "GET" {
                    serde_json::json!({"users": [{"id": 1, "login": "alice", "name": "Alice"}]})
                } else {
                    serde_json::json!({"ok": true, "msg": "owners updated"})
                };
                ("200 OK", body.to_string().into_bytes())
            } else if request.method == "PUT" && path_only == "/api/v1/crates/new" {
                (
                    "200 OK",
                    b"{\"warnings\":{\"invalid_categories\":[],\"invalid_badges\":[],\"other\":[]}}".to_vec(),
                )
            } else if path_only.ends_with("/yank") || path_only.ends_with("/unyank") {
                ("200 OK", b"{\"ok\":true}".to_vec())
            } else {
                match routes.get(&path_only) {
                    Some(body) => ("200 OK", body.clone()),
                    None => ("404 Not Found", b"{\"errors\":[{\"detail\":\"Not Found\"}]}".to_vec()),
                }
            };
            let _ = write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(&body);
        }
    });
    Registry {
        url: base,
        requests,
    }
}

fn read_request(stream: &mut std::net::TcpStream) -> Option<Request> {
    let mut data = Vec::new();
    let mut buf = [0u8; 8192];
    let header_end = loop {
        let n = stream.read(&mut buf).ok()?;
        if n == 0 {
            return None;
        }
        data.extend_from_slice(&buf[..n]);
        if let Some(pos) = data.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos + 4;
        }
    };
    let head = String::from_utf8_lossy(&data[..header_end]).to_string();
    let mut lines = head.lines();
    let first = lines.next()?;
    let mut words = first.split_whitespace();
    let method = words.next()?.to_string();
    let path = words.next()?.to_string();
    let mut headers = HashMap::new();
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    let length: usize = headers
        .get("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut body = data[header_end..].to_vec();
    while body.len() < length {
        let n = stream.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&buf[..n]);
    }
    Some(Request {
        method,
        path,
        headers,
        body,
    })
}
