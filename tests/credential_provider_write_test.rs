/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! `login` and `logout` go through the configured credential providers, as Cargo runs them:
//! the first provider that supports the operation takes it; `cargo:token` writes
//! credentials.toml; a provider that only reads (`cargo:token-from-stdout`) is passed over.
// The plugin is a shell script.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

/// A protocol-1 plugin that appends every request to `log` and accepts it.
fn plugin(dir: &Path, log: &Path) -> String {
    let path = dir.join("cargo-credential-test");
    fs::write(
        &path,
        format!(
            "#!/bin/sh\necho '{{\"v\":[1]}}'\nread -r req\necho \"$req\" >> '{}'\n\
             case \"$req\" in *'\"logout\"'*) echo '{{\"Ok\":{{\"kind\":\"logout\"}}}}' ;; \
             *) echo '{{\"Ok\":{{\"kind\":\"login\"}}}}' ;; esac\n",
            log.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path.display().to_string()
}

fn mitts(dir: &Path, cargo_home: &Path, args: &[&str]) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_mitts"))
        .args(args)
        .current_dir(dir)
        .env("CARGO_HOME", cargo_home)
        .env_remove("CARGO_REGISTRY_TOKEN")
        .env_remove("CARGO_REGISTRY_GLOBAL_CREDENTIAL_PROVIDERS")
        .output()
        .unwrap();
    (
        out.status.success(),
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
    )
}

#[test]
fn login_and_logout_go_to_the_first_provider_that_writes() {
    let temp = TempDir::new().unwrap();
    let cargo_home = temp.path().join("home");
    let dir = temp.path().join("proj");
    fs::create_dir_all(dir.join(".cargo")).unwrap();
    fs::create_dir_all(&cargo_home).unwrap();
    let log = temp.path().join("requests.log");
    let exe = plugin(temp.path(), &log);
    let credentials = cargo_home.join("credentials.toml");

    // A plugin first: it takes login and logout; credentials.toml stays untouched.
    fs::write(
        dir.join(".cargo/config.toml"),
        format!("[registry]\nglobal-credential-providers = [\"cargo:token\", \"{exe}\"]\n"),
    )
    .unwrap();
    let (ok, text) = mitts(&dir, &cargo_home, &["login", "s3cret-token"]);
    assert!(ok, "{text}");
    let requests = fs::read_to_string(&log).unwrap();
    assert!(
        requests.contains("\"kind\":\"login\"") && requests.contains("s3cret-token"),
        "{requests}"
    );
    assert!(
        !credentials.exists(),
        "the plugin, not the file, took the token"
    );
    let (ok, text) = mitts(&dir, &cargo_home, &["logout"]);
    assert!(ok, "{text}");
    assert!(fs::read_to_string(&log)
        .unwrap()
        .contains("\"kind\":\"logout\""));

    // A provider that only reads is passed over: the token lands in credentials.toml.
    fs::write(
        dir.join(".cargo/config.toml"),
        "[registry]\nglobal-credential-providers = [\"cargo:token\", \"cargo:token-from-stdout echo x\"]\n",
    )
    .unwrap();
    let (ok, text) = mitts(&dir, &cargo_home, &["login", "file-token"]);
    assert!(ok, "{text}");
    assert!(fs::read_to_string(&credentials)
        .unwrap()
        .contains("file-token"));
    let (ok, text) = mitts(&dir, &cargo_home, &["logout"]);
    assert!(ok, "{text}");
    assert!(!fs::read_to_string(&credentials)
        .unwrap()
        .contains("file-token"));
}
