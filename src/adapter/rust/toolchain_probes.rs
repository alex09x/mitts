/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Toolchain probes (`rustup which rustc`, `rustc -vV`, `rustc --print cfg`) and their
//! on-disk cache under `<cache root>/toolchain/`.
//!
//! Every Mitts invocation used to spawn these three processes serially before planning;
//! on macOS each costs 13–19 ms (exec plus dyld of `librustc_driver`), which was most of
//! a warm no-op build. The answers only change when the compiler binary changes or
//! rustup selects another toolchain, so they are keyed on the probed file's identity
//! (path, length, mtime) plus everything that steers rustup's choice: `RUSTUP_TOOLCHAIN`,
//! `RUSTUP_HOME`, the rustup `settings.toml` and the nearest `rust-toolchain` file.
//!
//! The cache is active only after [`set_cache_root`] (the builder calls it with the
//! resolved cache root); without a root every probe spawns, as before. A rustup proxy
//! (`~/.cargo/bin/rustc`) is never cached: its output follows the toolchain behind it,
//! whose identity the proxy path does not carry.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::RwLock;
use std::time::Instant;

/// Subdirectory of the cache root holding the probe entries.
pub const TOOLCHAIN_DIR: &str = "toolchain";

static CACHE_ROOT: RwLock<Option<PathBuf>> = RwLock::new(None);

/// Enables the on-disk caches under `cache_root` (`toolchain/`, `env_macros/`).
pub fn set_cache_root(cache_root: &Path) {
    *CACHE_ROOT.write().unwrap_or_else(|e| e.into_inner()) = Some(cache_root.to_path_buf());
}

/// The cache root the invocation runs against, once the builder has set it.
pub fn cache_root() -> Option<PathBuf> {
    CACHE_ROOT.read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Directory of the probe cache, when enabled.
pub fn cache_dir() -> Option<PathBuf> {
    cache_root().map(|root| root.join(TOOLCHAIN_DIR))
}

#[derive(Serialize, Deserialize)]
struct Entry {
    key: String,
    value: String,
}

/// Identity of a file as `path|len|mtime_nanos`; a missing file is `path|missing`.
pub fn file_identity(path: &Path) -> String {
    match fs::metadata(path) {
        Ok(meta) => {
            let mtime = meta
                .modified()
                .ok()
                .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            format!("{}|{}|{}", path.display(), meta.len(), mtime)
        }
        Err(_) => format!("{}|missing", path.display()),
    }
}

/// Everything that decides which toolchain rustup runs for a command started in `cwd`.
pub fn rustup_selection_key(cwd: &Path) -> String {
    let env = |name: &str| std::env::var_os(name).map(|v| v.to_string_lossy().into_owned());
    let rustup_home = env("RUSTUP_HOME")
        .map(PathBuf::from)
        .or_else(|| env("HOME").map(|h| PathBuf::from(h).join(".rustup")));
    let settings = rustup_home
        .map(|h| file_identity(&h.join("settings.toml")))
        .unwrap_or_default();
    let mut toolchain_file = String::from("none");
    for dir in cwd.ancestors() {
        for name in ["rust-toolchain.toml", "rust-toolchain"] {
            let candidate = dir.join(name);
            if let Ok(content) = fs::read(&candidate) {
                toolchain_file = format!(
                    "{}|{}",
                    candidate.display(),
                    crate::core::hash::Sha256::digest(&content)
                );
                break;
            }
        }
        if toolchain_file != "none" {
            break;
        }
    }
    format!(
        "RUSTUP_TOOLCHAIN={:?}|RUSTUP_HOME={:?}|settings={}|toolchain_file={}",
        env("RUSTUP_TOOLCHAIN"),
        env("RUSTUP_HOME"),
        settings,
        toolchain_file
    )
}

/// True when `rustc_path` is a rustup proxy: the same file as the `rustup` binary next to it.
pub fn is_rustup_proxy(rustc_path: &Path) -> bool {
    let Some(dir) = rustc_path.parent() else {
        return false;
    };
    let rustup = dir.join("rustup");
    let (Ok(a), Ok(b)) = (fs::metadata(rustc_path), fs::metadata(&rustup)) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if a.dev() == b.dev() && a.ino() == b.ino() {
            return true;
        }
    }
    a.len() == b.len() && a.modified().ok() == b.modified().ok()
}

/// Key for a probe of `rustc_path`: the binary's identity, the rustup selection and `extra`.
pub fn rustc_probe_key(rustc_path: &Path, extra: &str) -> String {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    format!(
        "{}|{}|{}",
        file_identity(rustc_path),
        rustup_selection_key(&cwd),
        extra
    )
}

/// Returns the cached value for `(kind, key)` or runs `probe`, storing a successful answer.
/// Without a cache root, or when the probed binary is a rustup proxy, it always probes.
pub fn load_or_probe<E>(
    kind: &str,
    key: &str,
    cacheable: bool,
    probe: impl FnOnce() -> Result<String, E>,
) -> Result<String, E> {
    let dir = match cache_dir() {
        Some(dir) if cacheable => dir,
        _ => return probe(),
    };
    let file = dir.join(format!(
        "{kind}-{}.json",
        &crate::core::hash::Sha256::digest(key.as_bytes())[..16]
    ));
    if let Some(value) = read_entry(&file, key) {
        return Ok(value);
    }
    let value = probe()?;
    write_entry(&dir, &file, key, &value);
    Ok(value)
}

fn read_entry(file: &Path, key: &str) -> Option<String> {
    let content = fs::read(file).ok()?;
    let entry: Entry = serde_json::from_slice(&content).ok()?;
    (entry.key == key).then_some(entry.value)
}

fn write_entry(dir: &Path, file: &Path, key: &str, value: &str) {
    let _ = fs::create_dir_all(dir);
    let tmp = dir.join(format!(
        ".{}.{}.tmp",
        file.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id()
    ));
    let entry = Entry {
        key: key.to_string(),
        value: value.to_string(),
    };
    let Ok(bytes) = serde_json::to_vec(&entry) else {
        return;
    };
    if fs::write(&tmp, bytes).is_ok() && fs::rename(&tmp, file).is_err() {
        let _ = fs::remove_file(&tmp);
    }
}

/// `rustup which rustc` through `rustup_bin`, cached on the rustup binary and its selection;
/// a cached path is trusted only while the file it names is unchanged.
pub fn rustup_which_rustc(rustup_bin: &Path) -> Option<PathBuf> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let key = format!(
        "{}|{}",
        file_identity(rustup_bin),
        rustup_selection_key(&cwd)
    );
    let probe = || -> Result<String, ()> {
        let t0 = Instant::now();
        let out = std::process::Command::new(rustup_bin)
            .arg("which")
            .arg("rustc")
            .output();
        super::compiler_resolve::record_rustc_subproc(t0.elapsed());
        let out = out.map_err(|_| ())?;
        if !out.status.success() {
            return Err(());
        }
        let path = PathBuf::from(String::from_utf8(out.stdout).map_err(|_| ())?.trim());
        if !path.is_file() {
            return Err(());
        }
        Ok(format!("{}\n{}", path.display(), file_identity(&path)))
    };
    let stored = load_or_probe("rustup-which", &key, true, probe).ok()?;
    let (path, identity) = stored.split_once('\n')?;
    let path = PathBuf::from(path);
    if file_identity(&path) == identity {
        return Some(path);
    }
    // The toolchain behind the cached answer changed (rustup update): ask again.
    let fresh = match cache_dir() {
        Some(dir) => {
            let _ = fs::remove_file(dir.join(format!(
                "rustup-which-{}.json",
                &crate::core::hash::Sha256::digest(key.as_bytes())[..16]
            )));
            load_or_probe("rustup-which", &key, true, probe).ok()?
        }
        None => probe().ok()?,
    };
    fresh.split_once('\n').map(|(p, _)| PathBuf::from(p))
}

/// Stdout of `<rustc_path> -vV`; `Err` carries the failure text.
pub fn rustc_verbose_version(rustc_path: &Path) -> Result<String, String> {
    load_or_probe(
        "rustc-vV",
        &rustc_probe_key(rustc_path, "vV"),
        !is_rustup_proxy(rustc_path),
        || run_rustc(rustc_path, &["-vV"]),
    )
}

/// Stdout of `<rustc_path> --print cfg [--target <target>]`; `Err` carries the failure text.
pub fn rustc_print_cfg(rustc_path: &Path, target: Option<&str>) -> Result<String, String> {
    let mut args = vec!["--print", "cfg"];
    if let Some(t) = target {
        args.extend(["--target", t]);
    }
    load_or_probe(
        "rustc-cfg",
        &rustc_probe_key(rustc_path, &format!("cfg|target={target:?}")),
        !is_rustup_proxy(rustc_path),
        || run_rustc(rustc_path, &args),
    )
}

fn run_rustc(rustc_path: &Path, args: &[&str]) -> Result<String, String> {
    let t0 = Instant::now();
    let output = std::process::Command::new(rustc_path).args(args).output();
    super::compiler_resolve::record_rustc_subproc(t0.elapsed());
    let output = output.map_err(|e| format!("Failed to invoke {rustc_path:?} {args:?}: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "{rustc_path:?} {args:?} failed with exit code {:?}: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    String::from_utf8(output.stdout)
        .map_err(|e| format!("{rustc_path:?} {args:?} output is not valid utf8: {e}"))
}
