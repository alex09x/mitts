/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Cargo's credential providers, as `cargo` runs them for a read: the built-in
//! `cargo:token` (credentials file and environment), `cargo:token-from-stdout <cmd>`,
//! `cargo:macos-keychain`, `cargo:libsecret`, and plugins speaking the credential
//! provider protocol (`{"v":[1]}` hello, one JSON request, one JSON reply). Providers
//! come from `[registries.<name>] credential-provider`, else
//! `[registry] global-credential-providers` (later entries first), with
//! `[credential-alias]` expanded; `cargo:token` alone when nothing is configured.

use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};

/// One provider as `cargo` would invoke it: `["cargo:token"]`, `["cargo-credential-1password",
/// "--account", "x"]`, `["cargo:token-from-stdout", "op", "read", "..."]`.
pub type ProviderSpec = Vec<String>;

/// Tokens already obtained in this process, by index URL and provider list.
static CACHE: OnceLock<Mutex<HashMap<String, Option<String>>>> = OnceLock::new();

pub struct RegistryRef<'a> {
    pub name: Option<&'a str>,
    /// The index as Cargo names it to providers (`sparse+https://…/`).
    pub index_url: &'a str,
}

/// Runs the providers in order until one yields a token. `Ok(None)`: none had one.
pub fn token_for(
    providers: &[ProviderSpec],
    registry: &RegistryRef<'_>,
    token_file_lookup: &dyn Fn() -> Option<String>,
) -> Result<Option<String>> {
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let cache_key = format!(
        "{}|{}",
        registry.index_url,
        providers
            .iter()
            .map(|p| p.join(" "))
            .collect::<Vec<_>>()
            .join(",")
    );
    if let Some(found) = cache.lock().unwrap().get(&cache_key) {
        return Ok(found.clone());
    }
    let mut result = None;
    for spec in providers {
        let Some(kind) = spec.first() else {
            continue;
        };
        let got = match kind.as_str() {
            "cargo:token" => token_file_lookup(),
            "cargo:token-from-stdout" => Some(token_from_stdout(&spec[1..], registry)?),
            "cargo:macos-keychain" => keychain(registry.index_url)?,
            "cargo:libsecret" => libsecret(registry.index_url)?,
            "cargo:wincred" => None,
            _ => plugin(spec, registry)?,
        };
        if let Some(t) = got.filter(|t| !t.is_empty()) {
            result = Some(t);
            break;
        }
    }
    cache.lock().unwrap().insert(cache_key, result.clone());
    Ok(result)
}

fn cargo_path() -> String {
    std::env::var("CARGO")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "cargo".to_string())
}

fn token_from_stdout(argv: &[String], registry: &RegistryRef<'_>) -> Result<String> {
    let Some((cmd, args)) = argv.split_first() else {
        bail!("`cargo:token-from-stdout` needs a command");
    };
    let mut c = Command::new(cmd);
    c.args(args)
        .env("CARGO", cargo_path())
        .env("CARGO_REGISTRY_INDEX_URL", registry.index_url)
        .stdin(Stdio::inherit())
        .stderr(Stdio::inherit());
    match registry.name {
        Some(n) => {
            c.env("CARGO_REGISTRY_NAME_OPT", n);
        }
        None => {
            c.env_remove("CARGO_REGISTRY_NAME_OPT");
        }
    }
    let out = c
        .output()
        .with_context(|| format!("running credential command `{cmd}`"))?;
    if !out.status.success() {
        bail!(
            "credential command `{cmd}` failed ({}) for registry {}",
            out.status,
            registry.index_url
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// `security find-generic-password -s cargo-registry:<index> -w`: the item Cargo's own
/// provider stores. A missing item is not an error.
fn keychain(index_url: &str) -> Result<Option<String>> {
    if !cfg!(target_os = "macos") {
        return Ok(None);
    }
    let out = Command::new("security")
        .args([
            "find-generic-password",
            "-s",
            &format!("cargo-registry:{index_url}"),
            "-w",
        ])
        .stdin(Stdio::null())
        .output();
    Ok(match out {
        Ok(o) if o.status.success() => Some(String::from_utf8_lossy(&o.stdout).trim().to_string()),
        _ => None,
    })
}

/// `secret-tool lookup url <index>`: the attribute Cargo's libsecret provider stores.
fn libsecret(index_url: &str) -> Result<Option<String>> {
    if cfg!(target_os = "macos") {
        return Ok(None);
    }
    let out = Command::new("secret-tool")
        .args(["lookup", "url", index_url])
        .stdin(Stdio::null())
        .output();
    Ok(match out {
        Ok(o) if o.status.success() => Some(String::from_utf8_lossy(&o.stdout).trim().to_string()),
        _ => None,
    })
}

/// A plugin following Cargo's credential provider protocol, version 1.
fn plugin(spec: &[String], registry: &RegistryRef<'_>) -> Result<Option<String>> {
    let (exe, args) = spec.split_first().expect("non-empty spec");
    let mut child = Command::new(exe)
        .arg("--cargo-plugin")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .with_context(|| format!("starting credential provider `{exe}`"))?;
    let mut stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");
    let mut lines = BufReader::new(stdout).lines();
    let hello = lines
        .next()
        .transpose()?
        .with_context(|| format!("credential provider `{exe}` sent no hello"))?;
    let hello: serde_json::Value = serde_json::from_str(&hello)
        .with_context(|| format!("credential provider `{exe}`: bad hello `{hello}`"))?;
    let supports_v1 = hello["v"]
        .as_array()
        .is_some_and(|v| v.iter().any(|x| x.as_u64() == Some(1)));
    if !supports_v1 {
        bail!("credential provider `{exe}` does not speak protocol version 1");
    }
    let mut reg = serde_json::json!({ "index-url": registry.index_url });
    if let Some(n) = registry.name {
        reg["name"] = serde_json::Value::String(n.to_string());
    }
    let request = serde_json::json!({
        "v": 1,
        "kind": "get",
        "operation": "read",
        "registry": reg,
        "args": args,
    });
    writeln!(stdin, "{request}")?;
    stdin.flush()?;
    let reply = lines
        .next()
        .transpose()?
        .with_context(|| format!("credential provider `{exe}` sent no reply"))?;
    drop(stdin);
    let _ = child.wait();
    let reply: serde_json::Value = serde_json::from_str(&reply)
        .with_context(|| format!("credential provider `{exe}`: bad reply `{reply}`"))?;
    if let Some(ok) = reply.get("Ok") {
        return Ok(ok["token"].as_str().map(str::to_string));
    }
    match reply["Err"]["kind"].as_str() {
        Some("not-found") | Some("url-not-supported") => Ok(None),
        Some(other) => bail!(
            "credential provider `{exe}` failed for {}: {other}{}",
            registry.index_url,
            reply["Err"]["message"]
                .as_str()
                .map(|m| format!(": {m}"))
                .unwrap_or_default()
        ),
        None => bail!("credential provider `{exe}`: unrecognised reply {reply}"),
    }
}

/// A config value that is a string (split on spaces) or an array of strings.
pub fn spec_from_toml(value: &toml::Value) -> Option<ProviderSpec> {
    match value {
        toml::Value::String(s) => Some(s.split_whitespace().map(str::to_string).collect()),
        toml::Value::Array(a) => Some(
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect(),
        ),
        _ => None,
    }
}
