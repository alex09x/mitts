/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

/// Checks whether a package is strictly exclusive to a foreign target platform.
pub fn is_target_incompatible_with_host(dep_name: &str) -> bool {
    #[cfg(not(target_os = "android"))]
    if matches!(
        dep_name,
        "ndk-sys" | "ndk" | "android-activity" | "android_system_properties"
    ) {
        return true;
    }
    #[cfg(not(target_os = "redox"))]
    if matches!(dep_name, "orbclient") {
        return true;
    }
    #[cfg(not(any(target_arch = "wasm32", target_arch = "wasm64")))]
    if matches!(
        dep_name,
        "web-sys"
            | "js-sys"
            | "wasm-bindgen"
            | "wasm-bindgen-futures"
            | "wasm-bindgen-test"
            | "wasm-bindgen-test-macro"
            | "wasm-bindgen-macro"
            | "wasm-bindgen-shared"
            | "wasm-bindgen-backend"
    ) || dep_name.starts_with("wasm-bindgen-")
    {
        return true;
    }
    #[cfg(not(target_os = "windows"))]
    {
        if matches!(
            dep_name,
            "glutin_wgl_sys"
                | "accesskit_windows"
                | "windows"
                | "windows-future"
                | "windows-collections"
                | "windows-numerics"
                | "windows-threading"
                | "schannel"
                | "winreg"
                | "ntapi"
                | "crossterm_winapi"
                | "anstyle-wincon"
        ) || dep_name.starts_with("windows_")
            || dep_name.starts_with("windows-")
            || dep_name.starts_with("winapi")
            || dep_name.starts_with("crossterm_win")
        {
            return true;
        }
    }
    #[cfg(not(any(
        target_os = "linux",
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd"
    )))]
    {
        if matches!(
            dep_name,
            "accesskit_unix"
                | "accesskit_atspi_common"
                | "glutin_glx_sys"
                | "calloop-wayland-source"
                | "sctk-adwaita"
                | "smithay-client-toolkit"
                | "smithay-clipboard"
                | "xkbcommon-dl"
                | "x11-dl"
                | "x11rb"
                | "x11rb-protocol"
        ) || dep_name.starts_with("wayland-")
            || dep_name.starts_with("smithay-")
        {
            return true;
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "ios")))]
    {
        if matches!(
            dep_name,
            "mach2"
                | "core-foundation"
                | "core-foundation-sys"
                | "core-graphics"
                | "core-graphics-types"
                | "core-text"
                | "core-audio"
                | "core-audio-types"
                | "security-framework"
                | "security-framework-sys"
                | "fsevent-sys"
                | "kqueue"
                | "kqueue-sys"
                | "metal"
                | "block2"
                | "objc2"
                | "dispatch"
        ) || dep_name.starts_with("objc2-")
            || dep_name.starts_with("apple-")
        {
            return true;
        }
    }
    false
}

/// Checks whether a target configuration spec (e.g. `'cfg(windows)'` or `cfg(target_env = "musl")`)
/// or target triple is incompatible with the current compilation host.
pub fn is_target_cfg_incompatible_with_host(target_spec: &str) -> bool {
    is_target_cfg_incompatible_with_target(target_spec, false).unwrap_or(false)
}

pub fn is_target_cfg_incompatible_with_target(
    target_spec: &str,
    for_host: bool,
) -> Result<bool, RustAdapterError> {
    let clean = target_spec
        .trim()
        .trim_matches('\'')
        .trim_matches('"')
        .trim();

    if !clean.starts_with("cfg(") {
        #[cfg(not(target_os = "windows"))]
        if clean.contains("windows") || clean.contains("msvc") || clean.contains("win32") {
            return Ok(true);
        }
        #[cfg(not(target_os = "linux"))]
        if clean.contains("linux") || clean.contains("android") {
            return Ok(true);
        }
        #[cfg(not(any(target_os = "macos", target_os = "ios")))]
        if clean.contains("apple") || clean.contains("darwin") || clean.contains("ios") {
            return Ok(true);
        }
        #[cfg(not(any(target_arch = "wasm32", target_arch = "wasm64")))]
        if clean.contains("wasm32") || clean.contains("wasm64") {
            return Ok(true);
        }
        #[cfg(not(target_env = "musl"))]
        if clean.contains("musl") {
            return Ok(true);
        }
        return Ok(false);
    }

    eval_cfg(clean, for_host).map(|res| !res)
}

fn eval_cfg(expr: &str, for_host: bool) -> Result<bool, RustAdapterError> {
    let expr = expr.trim();
    if let Some(inner) = expr.strip_prefix("cfg(").and_then(|s| s.strip_suffix(')')) {
        return eval_cfg(inner, for_host);
    }
    if let Some(inner) = expr.strip_prefix("not(").and_then(|s| s.strip_suffix(')')) {
        return eval_cfg(inner, for_host).map(|b| !b);
    }
    if let Some(inner) = expr.strip_prefix("any(").and_then(|s| s.strip_suffix(')')) {
        for arg in split_cfg_args(inner) {
            if eval_cfg(arg, for_host)? {
                return Ok(true);
            }
        }
        return Ok(false);
    }
    if let Some(inner) = expr.strip_prefix("all(").and_then(|s| s.strip_suffix(')')) {
        for arg in split_cfg_args(inner) {
            if !eval_cfg(arg, for_host)? {
                return Ok(false);
            }
        }
        return Ok(true);
    }
    eval_atom(expr, for_host)
}

fn split_cfg_args(s: &str) -> Vec<&str> {
    let mut args = Vec::new();
    let mut depth = 0;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                if depth > 0 {
                    depth -= 1;
                }
            }
            ',' if depth == 0 => {
                let trimmed = s[start..i].trim();
                if !trimmed.is_empty() {
                    args.push(trimmed);
                }
                start = i + 1;
            }
            _ => {}
        }
    }
    let last = s[start..].trim();
    if !last.is_empty() {
        args.push(last);
    }
    args
}

use super::types::RustAdapterError;
use std::collections::HashSet;
use std::sync::OnceLock;

static HOST_CFG_ATOMS: OnceLock<Result<HashSet<String>, String>> = OnceLock::new();
static TARGET_CFG_ATOMS: OnceLock<Result<HashSet<String>, String>> = OnceLock::new();
static EMPTY_ATOMS: OnceLock<HashSet<String>> = OnceLock::new();

pub fn get_cfg_atoms(for_host: bool) -> Result<&'static HashSet<String>, RustAdapterError> {
    let lock = if for_host {
        &HOST_CFG_ATOMS
    } else {
        &TARGET_CFG_ATOMS
    };
    let res = lock.get_or_init(|| discover_cfg_atoms(for_host));
    match res {
        Ok(atoms) => Ok(atoms),
        Err(err) => Err(RustAdapterError::MetadataFailed(err.clone())),
    }
}

pub fn get_host_cfg_atoms() -> &'static HashSet<String> {
    get_cfg_atoms(true).unwrap_or_else(|_| EMPTY_ATOMS.get_or_init(HashSet::new))
}

fn discover_cfg_atoms(for_host: bool) -> Result<HashSet<String>, String> {
    let rustc = crate::adapter::rust::compiler_resolve::resolve_rustc();
    let mut cmd = std::process::Command::new(&rustc);
    cmd.arg("--print").arg("cfg");

    let explicit_target = if !for_host {
        crate::adapter::rust::key_flags::resolve_explicit_target_from_args(&[])
    } else {
        None
    };

    if let Some(ref target) = explicit_target {
        cmd.arg("--target").arg(target);
    }

    let output = cmd.output().map_err(|e| {
        format!("Failed to execute {:?} to discover cfg atoms: {}", rustc, e)
    })?;

    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "rustc --print cfg failed with exit code {:?}:\n{}",
            output.status.code(),
            err
        ));
    }

    let text = std::str::from_utf8(&output.stdout)
        .map_err(|e| format!("rustc cfg output not valid utf8: {}", e))?;

    let mut atoms = HashSet::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            atoms.insert(trimmed.to_string());
        }
    }

    if let Ok(encoded) = std::env::var("CARGO_ENCODED_RUSTFLAGS") {
        parse_cfg_from_iter(encoded.split('\x1f'), &mut atoms);
    } else if let Ok(flags) = std::env::var("RUSTFLAGS") {
        parse_cfg_from_iter(flags.split_whitespace(), &mut atoms);
    }

    if let Ok(cfg_flags) = crate::adapter::rust::cargo_config::load_cargo_config_rustflags(
        std::path::Path::new("."),
        explicit_target.as_deref(),
    ) {
        parse_cfg_from_iter(cfg_flags.iter().map(|s| s.as_str()), &mut atoms);
    }

    Ok(atoms)
}

fn parse_cfg_from_iter<'a>(iter: impl Iterator<Item = &'a str>, atoms: &mut HashSet<String>) {
    let mut it = iter;
    while let Some(arg) = it.next() {
        if arg == "--cfg" {
            if let Some(val) = it.next() {
                atoms.insert(val.to_string());
            }
        } else if let Some(val) = arg.strip_prefix("--cfg=") {
            atoms.insert(val.to_string());
        }
    }
}

fn eval_atom(atom: &str, for_host: bool) -> Result<bool, RustAdapterError> {
    let atom = atom.trim();
    let atoms = get_cfg_atoms(for_host)?;
    if atoms.contains(atom) {
        return Ok(true);
    }
    if let Some((k, v)) = atom.split_once('=') {
        let key = k.trim();
        let val = v.trim().trim_matches('"').trim_matches('\'');
        let normalized = format!("{key}=\"{val}\"");
        return Ok(atoms.contains(&normalized));
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_eval_cfg_target_atoms() {
        assert!(eval_atom("target_has_atomic = \"64\"", false).unwrap() || eval_atom("target_has_atomic = \"32\"", false).unwrap());
        assert!(!eval_atom("target_os = \"nonexistent_os\"", false).unwrap());
        assert!(!is_target_cfg_incompatible_with_host("cfg(any(unix, windows))"));
        assert!(is_target_cfg_incompatible_with_host("cfg(target_os = \"nonexistent_os\")"));
    }
}

