/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Cargo's credential providers, driven exactly as `cargo` drives them: the OS keychain
//! (`security` on macOS, `secret-tool` on Linux, both faked on PATH here), a command whose
//! stdout is the token, a plugin speaking the credential provider protocol, aliases, and the
//! precedence rule that later entries of `global-credential-providers` are tried first.

#![cfg(unix)]

use mitts::adapter::rust::registry_config::RegistryConfig;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tempfile::TempDir;

/// Tests mutate the process environment and PATH: run them one at a time.
static ENV_LOCK: Mutex<()> = Mutex::new(());

const INDEX: &str = "sparse+https://reg.example/idx/";

fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, format!("#!/bin/sh\n{body}")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// Fake binaries on PATH: `security` and `secret-tool` answer for our index only.
fn fake_bin(dir: &Path) -> PathBuf {
    let bin = dir.join("bin");
    fs::create_dir_all(&bin).unwrap();
    script(
        &bin,
        "security",
        &format!(
            "[ \"$1\" = find-generic-password ] && [ \"$3\" = \"cargo-registry:{INDEX}\" ] && {{ echo keychain-tok; exit 0; }}\nexit 44\n"
        ),
    );
    script(
        &bin,
        "secret-tool",
        &format!(
            "[ \"$1\" = lookup ] && [ \"$2\" = url ] && [ \"$3\" = \"{INDEX}\" ] && {{ echo libsecret-tok; exit 0; }}\nexit 1\n"
        ),
    );
    // A plugin that answers the protocol: hello, then one reply per request.
    script(
        &bin,
        "cargo-credential-fake",
        &format!(
            "[ \"$1\" = --cargo-plugin ] || exit 2\necho '{{\"v\":[1]}}'\nread -r req\ncase \"$req\" in\n  *\"{INDEX}\"*) echo '{{\"Ok\":{{\"kind\":\"get\",\"token\":\"plugin-tok\",\"cache\":\"session\",\"operation_independent\":true}}}}' ;;\n  *) echo '{{\"Err\":{{\"kind\":\"not-found\"}}}}' ;;\nesac\n"
        ),
    );
    script(
        &bin,
        "print-token",
        "echo \"stdout-tok:$CARGO_REGISTRY_INDEX_URL:$CARGO_REGISTRY_NAME_OPT\"\n",
    );
    bin
}

struct Env {
    saved: Vec<(String, Option<String>)>,
}

impl Env {
    fn set(vars: &[(&str, &str)]) -> Self {
        let saved = vars
            .iter()
            .map(|(k, v)| {
                let old = std::env::var(k).ok();
                std::env::set_var(k, v);
                (k.to_string(), old)
            })
            .collect();
        Self { saved }
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        for (k, old) in self.saved.drain(..) {
            match old {
                Some(v) => std::env::set_var(&k, v),
                None => std::env::remove_var(&k),
            }
        }
    }
}

fn config_with(temp: &Path, providers_toml: &str) -> RegistryConfig {
    let project = temp.join("proj");
    fs::create_dir_all(project.join(".cargo")).unwrap();
    fs::write(
        project.join(".cargo/config.toml"),
        format!("[registries]\nreg = {{ index = \"{INDEX}\" }}\n{providers_toml}"),
    )
    .unwrap();
    RegistryConfig::load(&project).unwrap()
}

fn os_provider() -> (&'static str, &'static str) {
    if cfg!(target_os = "macos") {
        ("cargo:macos-keychain", "keychain-tok")
    } else {
        ("cargo:libsecret", "libsecret-tok")
    }
}

#[test]
fn the_os_keychain_provider_asks_the_system_tool_for_the_registry_item() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = TempDir::new().unwrap();
    let bin = fake_bin(temp.path());
    let cargo_home = temp.path().join("cargo_home");
    fs::create_dir_all(&cargo_home).unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let _env = Env::set(&[
        ("PATH", &path),
        ("CARGO_HOME", cargo_home.to_str().unwrap()),
    ]);
    let (provider, expected) = os_provider();
    // A distinct index per test so the process-wide token cache cannot interfere.
    let cfg = config_with(
        temp.path(),
        &format!("[registry]\nglobal-credential-providers = [\"cargo:token\", \"{provider}\"]\n"),
    );
    assert_eq!(
        cfg.token_for_index(INDEX).unwrap().as_deref(),
        Some(expected)
    );
}

#[test]
fn later_global_providers_win_and_aliases_and_stdout_commands_work() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = TempDir::new().unwrap();
    let bin = fake_bin(temp.path());
    let cargo_home = temp.path().join("cargo_home");
    fs::create_dir_all(&cargo_home).unwrap();
    fs::write(
        cargo_home.join("credentials.toml"),
        "[registries.reg]\ntoken = \"file-tok\"\n",
    )
    .unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let _env = Env::set(&[
        ("PATH", &path),
        ("CARGO_HOME", cargo_home.to_str().unwrap()),
    ]);

    // Order only: the file token is listed last, so it is tried first.
    let cfg = config_with(
        temp.path(),
        "[registry]\nglobal-credential-providers = [\"cargo-credential-fake\", \"cargo:token\"]\n",
    );
    let specs = cfg.providers_for(Some("reg"));
    assert_eq!(specs[0], vec!["cargo:token"]);
    assert_eq!(specs[1], vec!["cargo-credential-fake"]);

    // A registry-specific provider replaces the global list; an alias expands.
    let project = temp.path().join("proj2");
    fs::create_dir_all(project.join(".cargo")).unwrap();
    fs::write(
        project.join(".cargo/config.toml"),
        format!(
            "[registries]\nreg = {{ index = \"{INDEX}\", credential-provider = \"from-stdout\" }}\n[registry]\nglobal-credential-providers = [\"cargo:token\"]\n[credential-alias]\nfrom-stdout = [\"cargo:token-from-stdout\", \"print-token\"]\n"
        ),
    )
    .unwrap();
    let cfg = RegistryConfig::load(&project).unwrap();
    assert_eq!(
        cfg.providers_for(Some("reg")),
        vec![vec!["cargo:token-from-stdout", "print-token"]]
    );
    // The command sees the index URL and the registry name, as Cargo passes them.
    assert_eq!(
        cfg.token_for_index(INDEX).unwrap().as_deref(),
        Some(&format!("stdout-tok:{INDEX}:reg")[..])
    );
}

#[test]
fn a_protocol_plugin_answers_for_its_registry_and_defers_otherwise() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = TempDir::new().unwrap();
    let bin = fake_bin(temp.path());
    let cargo_home = temp.path().join("cargo_home");
    fs::create_dir_all(&cargo_home).unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let _env = Env::set(&[
        ("PATH", &path),
        ("CARGO_HOME", cargo_home.to_str().unwrap()),
    ]);
    let other = "sparse+https://other.example/idx/";
    let project = temp.path().join("proj");
    fs::create_dir_all(project.join(".cargo")).unwrap();
    fs::write(
        project.join(".cargo/config.toml"),
        format!(
            "[registries]\nplug = {{ index = \"{other}\", credential-provider = [\"cargo-credential-fake\", \"--flag\"] }}\nmine = {{ index = \"{INDEX}\", credential-provider = \"cargo-credential-fake\" }}\n"
        ),
    )
    .unwrap();
    let cfg = RegistryConfig::load(&project).unwrap();
    // The fake plugin knows our index and reports `not-found` for any other.
    assert_eq!(
        cfg.token_for_index(INDEX).unwrap().as_deref(),
        Some("plugin-tok")
    );
    assert_eq!(cfg.token_for_index(other).unwrap(), None);
}

#[test]
fn without_configuration_only_the_token_file_and_environment_are_used() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = TempDir::new().unwrap();
    let cargo_home = temp.path().join("cargo_home");
    fs::create_dir_all(&cargo_home).unwrap();
    let _env = Env::set(&[
        ("CARGO_HOME", cargo_home.to_str().unwrap()),
        ("CARGO_REGISTRIES_ENVREG_TOKEN", "env-tok"),
    ]);
    let index = "sparse+https://env.example/idx/";
    let project = temp.path().join("proj");
    fs::create_dir_all(project.join(".cargo")).unwrap();
    fs::write(
        project.join(".cargo/config.toml"),
        format!("[registries]\nenvreg = {{ index = \"{index}\" }}\n"),
    )
    .unwrap();
    let cfg = RegistryConfig::load(&project).unwrap();
    assert_eq!(cfg.providers_for(Some("envreg")), vec![vec!["cargo:token"]]);
    assert_eq!(
        cfg.token_for_index(index).unwrap().as_deref(),
        Some("env-tok")
    );
}
