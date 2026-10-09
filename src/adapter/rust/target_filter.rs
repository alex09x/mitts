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
    let clean = target_spec
        .trim()
        .trim_matches('\'')
        .trim_matches('"')
        .trim();

    if !clean.starts_with("cfg(") {
        #[cfg(not(target_os = "windows"))]
        if clean.contains("windows") || clean.contains("msvc") || clean.contains("win32") {
            return true;
        }
        #[cfg(not(target_os = "linux"))]
        if clean.contains("linux") || clean.contains("android") {
            return true;
        }
        #[cfg(not(any(target_os = "macos", target_os = "ios")))]
        if clean.contains("apple") || clean.contains("darwin") || clean.contains("ios") {
            return true;
        }
        #[cfg(not(any(target_arch = "wasm32", target_arch = "wasm64")))]
        if clean.contains("wasm32") || clean.contains("wasm64") {
            return true;
        }
        #[cfg(not(target_env = "musl"))]
        if clean.contains("musl") {
            return true;
        }
        return false;
    }

    !eval_cfg(clean)
}

fn eval_cfg(expr: &str) -> bool {
    let expr = expr.trim();
    if let Some(inner) = expr.strip_prefix("cfg(").and_then(|s| s.strip_suffix(')')) {
        return eval_cfg(inner);
    }
    if let Some(inner) = expr.strip_prefix("not(").and_then(|s| s.strip_suffix(')')) {
        return !eval_cfg(inner);
    }
    if let Some(inner) = expr.strip_prefix("any(").and_then(|s| s.strip_suffix(')')) {
        return split_cfg_args(inner).into_iter().any(eval_cfg);
    }
    if let Some(inner) = expr.strip_prefix("all(").and_then(|s| s.strip_suffix(')')) {
        return split_cfg_args(inner).into_iter().all(eval_cfg);
    }
    eval_atom(expr)
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

use std::collections::HashSet;
use std::sync::OnceLock;

static HOST_CFG_ATOMS: OnceLock<HashSet<String>> = OnceLock::new();

pub fn get_host_cfg_atoms() -> &'static HashSet<String> {
    HOST_CFG_ATOMS.get_or_init(|| {
        let mut atoms = HashSet::new();
        if let Ok(output) = std::process::Command::new("rustc")
            .arg("--print")
            .arg("cfg")
            .output()
        {
            if output.status.success() {
                if let Ok(text) = std::str::from_utf8(&output.stdout) {
                    for line in text.lines() {
                        let trimmed = line.trim();
                        if !trimmed.is_empty() {
                            atoms.insert(trimmed.to_string());
                        }
                    }
                }
            }
        }
        if atoms.is_empty() {
            #[cfg(unix)]
            atoms.insert("unix".to_string());
            #[cfg(windows)]
            atoms.insert("windows".to_string());
            #[cfg(target_os = "linux")]
            atoms.insert("target_os=\"linux\"".to_string());
            #[cfg(target_os = "macos")]
            atoms.insert("target_os=\"macos\"".to_string());
            #[cfg(target_os = "windows")]
            atoms.insert("target_os=\"windows\"".to_string());
            #[cfg(target_arch = "x86_64")]
            atoms.insert("target_arch=\"x86_64\"".to_string());
            #[cfg(target_arch = "aarch64")]
            atoms.insert("target_arch=\"aarch64\"".to_string());
            #[cfg(target_pointer_width = "64")]
            atoms.insert("target_pointer_width=\"64\"".to_string());
            #[cfg(target_pointer_width = "32")]
            atoms.insert("target_pointer_width=\"32\"".to_string());
            atoms.insert("target_has_atomic=\"8\"".to_string());
            atoms.insert("target_has_atomic=\"16\"".to_string());
            atoms.insert("target_has_atomic=\"32\"".to_string());
            atoms.insert("target_has_atomic=\"64\"".to_string());
            atoms.insert("target_has_atomic=\"ptr\"".to_string());
        }
        atoms
    })
}

fn eval_atom(atom: &str) -> bool {
    let atom = atom.trim();
    let atoms = get_host_cfg_atoms();
    if atoms.contains(atom) {
        return true;
    }
    if let Some((k, v)) = atom.split_once('=') {
        let key = k.trim();
        let val = v.trim().trim_matches('"').trim_matches('\'');
        let normalized = format!("{key}=\"{val}\"");
        return atoms.contains(&normalized);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_eval_cfg_target_atoms() {
        assert!(eval_atom("target_has_atomic = \"64\"") || eval_atom("target_has_atomic = \"32\""));
        assert!(!eval_atom("target_os = \"nonexistent_os\""));
        assert!(!is_target_cfg_incompatible_with_host("cfg(any(unix, windows))"));
        assert!(is_target_cfg_incompatible_with_host("cfg(target_os = \"nonexistent_os\")"));
    }
}

