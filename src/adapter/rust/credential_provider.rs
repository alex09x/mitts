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
//! `cargo:macos-keychain`, `cargo:libsecret`, `cargo:wincred`, and plugins speaking the credential
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
            "cargo:wincred" => wincred(registry.index_url),
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

/// The Windows Credential Manager entry Cargo's wincred provider stores: a generic
/// credential named `cargo-registry:<index>` whose blob is the token in UTF-8. None
/// elsewhere, and when there is no such entry.
#[cfg(windows)]
fn wincred(index_url: &str) -> Option<String> {
    use std::ffi::c_void;
    #[repr(C)]
    struct CredentialW {
        flags: u32,
        kind: u32,
        target_name: *mut u16,
        comment: *mut u16,
        last_written: [u32; 2],
        blob_size: u32,
        blob: *mut u8,
        persist: u32,
        attribute_count: u32,
        attributes: *mut c_void,
        target_alias: *mut u16,
        user_name: *mut u16,
    }
    #[link(name = "advapi32")]
    extern "system" {
        fn CredReadW(target: *const u16, kind: u32, flags: u32, out: *mut *mut CredentialW) -> i32;
        fn CredFree(buffer: *mut c_void);
    }
    const CRED_TYPE_GENERIC: u32 = 1;
    let target: Vec<u16> = format!("cargo-registry:{index_url}")
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let mut cred: *mut CredentialW = std::ptr::null_mut();
    // SAFETY: `target` is NUL-terminated; a successful read returns a buffer that is
    // freed with CredFree after its blob is copied.
    unsafe {
        if CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut cred) == 0 || cred.is_null() {
            return None;
        }
        let c = &*cred;
        let token = if c.blob.is_null() {
            None
        } else {
            let bytes = std::slice::from_raw_parts(c.blob, c.blob_size as usize);
            Some(String::from_utf8_lossy(bytes).trim().to_string())
        };
        CredFree(cred.cast());
        token
    }
}

#[cfg(not(windows))]
fn wincred(_index_url: &str) -> Option<String> {
    None
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

/// `login` (`token`) or `logout` (`None`) through the providers, as Cargo runs them: the
/// first provider that supports the operation handles it; `cargo:token` writes the
/// credentials file through `write_file`. Whether a token was stored or removed.
pub fn store(
    providers: &[ProviderSpec],
    registry: &RegistryRef<'_>,
    token: Option<&str>,
    write_file: &dyn Fn(Option<&str>) -> Result<bool>,
) -> Result<bool> {
    // Tokens read in this process may change now.
    if let Some(cache) = CACHE.get() {
        cache.lock().unwrap().clear();
    }
    for spec in providers {
        let Some(kind) = spec.first() else {
            continue;
        };
        let handled = match kind.as_str() {
            "cargo:token" => Some(write_file(token)?),
            // Reads only, as in Cargo.
            "cargo:token-from-stdout" => None,
            "cargo:macos-keychain" => keychain_store(registry.index_url, token)?,
            "cargo:libsecret" => libsecret_store(registry.index_url, token)?,
            "cargo:wincred" => wincred_store(registry.index_url, token)?,
            _ => plugin_store(spec, registry, token)?,
        };
        if let Some(done) = handled {
            return Ok(done);
        }
    }
    bail!(
        "no credential provider configured for {} supports {}",
        registry.index_url,
        if token.is_some() { "login" } else { "logout" }
    )
}

fn keychain_store(index_url: &str, token: Option<&str>) -> Result<Option<bool>> {
    if !cfg!(target_os = "macos") {
        return Ok(None);
    }
    let service = format!("cargo-registry:{index_url}");
    let out = match token {
        Some(t) => Command::new("security")
            .args([
                "add-generic-password",
                "-U",
                "-a",
                "",
                "-s",
                &service,
                "-w",
                t,
            ])
            .stdin(Stdio::null())
            .output(),
        None => Command::new("security")
            .args(["delete-generic-password", "-s", &service])
            .stdin(Stdio::null())
            .output(),
    }
    .context("running `security`")?;
    match (token, out.status.success()) {
        (Some(_), true) => Ok(Some(true)),
        (Some(_), false) => bail!(
            "storing the token in the keychain failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ),
        // `delete-generic-password` fails when there is nothing to delete.
        (None, removed) => Ok(Some(removed)),
    }
}

fn libsecret_store(index_url: &str, token: Option<&str>) -> Result<Option<bool>> {
    if cfg!(target_os = "macos") || cfg!(windows) {
        return Ok(None);
    }
    match token {
        Some(t) => {
            let mut child = Command::new("secret-tool")
                .args([
                    "store",
                    &format!("--label=cargo-registry:{index_url}"),
                    "url",
                    index_url,
                ])
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .context("running `secret-tool`")?;
            child
                .stdin
                .take()
                .expect("piped stdin")
                .write_all(t.as_bytes())?;
            let out = child.wait_with_output()?;
            if !out.status.success() {
                bail!(
                    "storing the token with secret-tool failed: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                );
            }
            Ok(Some(true))
        }
        None => {
            let had = libsecret(index_url)?.is_some();
            let out = Command::new("secret-tool")
                .args(["clear", "url", index_url])
                .stdin(Stdio::null())
                .output()
                .context("running `secret-tool`")?;
            Ok(Some(had && out.status.success()))
        }
    }
}

#[cfg(windows)]
fn wincred_store(index_url: &str, token: Option<&str>) -> Result<Option<bool>> {
    use std::ffi::c_void;
    #[repr(C)]
    struct CredentialW {
        flags: u32,
        kind: u32,
        target_name: *const u16,
        comment: *const u16,
        last_written: [u32; 2],
        blob_size: u32,
        blob: *const u8,
        persist: u32,
        attribute_count: u32,
        attributes: *mut c_void,
        target_alias: *const u16,
        user_name: *const u16,
    }
    #[link(name = "advapi32")]
    extern "system" {
        fn CredWriteW(credential: *const CredentialW, flags: u32) -> i32;
        fn CredDeleteW(target: *const u16, kind: u32, flags: u32) -> i32;
    }
    const CRED_TYPE_GENERIC: u32 = 1;
    const CRED_PERSIST_LOCAL_MACHINE: u32 = 2;
    let wide = |s: &str| -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() };
    let target = wide(&format!("cargo-registry:{index_url}"));
    match token {
        Some(t) => {
            let user = wide("token");
            let cred = CredentialW {
                flags: 0,
                kind: CRED_TYPE_GENERIC,
                target_name: target.as_ptr(),
                comment: std::ptr::null(),
                last_written: [0, 0],
                blob_size: t.len() as u32,
                blob: t.as_ptr(),
                persist: CRED_PERSIST_LOCAL_MACHINE,
                attribute_count: 0,
                attributes: std::ptr::null_mut(),
                target_alias: std::ptr::null(),
                user_name: user.as_ptr(),
            };
            // SAFETY: every pointer refers to a live buffer for the duration of the call.
            if unsafe { CredWriteW(&cred, 0) } == 0 {
                bail!(
                    "storing the token in the Windows Credential Manager failed: {}",
                    std::io::Error::last_os_error()
                );
            }
            Ok(Some(true))
        }
        // SAFETY: `target` is NUL-terminated.
        None => Ok(Some(
            unsafe { CredDeleteW(target.as_ptr(), CRED_TYPE_GENERIC, 0) } != 0,
        )),
    }
}

#[cfg(not(windows))]
fn wincred_store(_index_url: &str, _token: Option<&str>) -> Result<Option<bool>> {
    Ok(None)
}

/// `login` / `logout` through a protocol-1 plugin; `None` when it does not support it.
fn plugin_store(
    spec: &[String],
    registry: &RegistryRef<'_>,
    token: Option<&str>,
) -> Result<Option<bool>> {
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
    let _hello = lines
        .next()
        .transpose()?
        .with_context(|| format!("credential provider `{exe}` sent no hello"))?;
    let mut reg = serde_json::json!({ "index-url": registry.index_url });
    if let Some(n) = registry.name {
        reg["name"] = serde_json::Value::String(n.to_string());
    }
    let kind = if token.is_some() { "login" } else { "logout" };
    let mut request = serde_json::json!({
        "v": 1,
        "kind": kind,
        "registry": reg,
        "args": args,
    });
    if let Some(t) = token {
        request["token"] = serde_json::Value::String(t.to_string());
    }
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
    if reply.get("Ok").is_some() {
        return Ok(Some(true));
    }
    match reply["Err"]["kind"].as_str() {
        Some("operation-not-supported") | Some("url-not-supported") => Ok(None),
        Some("not-found") if token.is_none() => Ok(Some(false)),
        Some(other) => bail!(
            "credential provider `{exe}` failed to {kind} for {}: {other}{}",
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

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::ffi::c_void;

    #[repr(C)]
    struct CredentialW {
        flags: u32,
        kind: u32,
        target_name: *const u16,
        comment: *const u16,
        last_written: [u32; 2],
        blob_size: u32,
        blob: *const u8,
        persist: u32,
        attribute_count: u32,
        attributes: *mut c_void,
        target_alias: *const u16,
        user_name: *const u16,
    }
    #[link(name = "advapi32")]
    extern "system" {
        fn CredWriteW(credential: *const CredentialW, flags: u32) -> i32;
        fn CredDeleteW(target: *const u16, kind: u32, flags: u32) -> i32;
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// Stored the way Cargo's wincred provider stores it; read back through `token_for`.
    #[test]
    fn a_wincred_token_is_read_from_the_credential_manager() {
        let index = format!(
            "sparse+https://mitts-wincred-test-{}.invalid/index/",
            std::process::id()
        );
        let target = wide(&format!("cargo-registry:{index}"));
        let user = wide("token");
        let secret = b"wincred-secret-token";
        let cred = CredentialW {
            flags: 0,
            kind: 1,
            target_name: target.as_ptr(),
            comment: std::ptr::null(),
            last_written: [0, 0],
            blob_size: secret.len() as u32,
            blob: secret.as_ptr(),
            persist: 2,
            attribute_count: 0,
            attributes: std::ptr::null_mut(),
            target_alias: std::ptr::null(),
            user_name: user.as_ptr(),
        };
        assert_ne!(unsafe { CredWriteW(&cred, 0) }, 0, "CredWriteW");
        let registry = RegistryRef {
            name: None,
            index_url: &index,
        };
        let got = token_for(&[vec!["cargo:wincred".to_string()]], &registry, &|| None);
        unsafe { CredDeleteW(target.as_ptr(), 1, 0) };
        assert_eq!(got.unwrap().as_deref(), Some("wincred-secret-token"));

        let missing = RegistryRef {
            name: None,
            index_url: "sparse+https://mitts-wincred-none.invalid/index/",
        };
        assert_eq!(
            token_for(&[vec!["cargo:wincred".to_string()]], &missing, &|| None).unwrap(),
            None
        );
    }

    /// `login` and `logout` with `cargo:wincred` write and delete the same entry.
    #[test]
    fn wincred_login_and_logout_round_trip() {
        let index = format!(
            "sparse+https://mitts-wincred-login-{}.invalid/index/",
            std::process::id()
        );
        let registry = RegistryRef {
            name: None,
            index_url: &index,
        };
        let providers = [vec!["cargo:wincred".to_string()]];
        let file = |_: Option<&str>| -> Result<bool> { panic!("the file is not used") };
        assert!(store(&providers, &registry, Some("login-token"), &file).unwrap());
        assert_eq!(
            token_for(&providers, &registry, &|| None)
                .unwrap()
                .as_deref(),
            Some("login-token")
        );
        assert!(store(&providers, &registry, None, &file).unwrap());
        CACHE.get().unwrap().lock().unwrap().clear();
        assert_eq!(token_for(&providers, &registry, &|| None).unwrap(), None);
    }
}
