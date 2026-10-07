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

fn eval_atom(atom: &str) -> bool {
    let atom = atom.trim();
    if atom == "windows" {
        return cfg!(target_os = "windows");
    }
    if atom == "unix" {
        return cfg!(unix);
    }
    if let Some((k, v)) = atom.split_once('=') {
        let key = k.trim();
        let val = v.trim().trim_matches('"').trim_matches('\'');
        return match key {
            "target_os" => host_target_os() == val,
            "target_arch" => host_target_arch() == val,
            "target_env" => host_target_env() == val,
            "target_family" => host_target_family() == val,
            "target_vendor" => host_target_vendor() == val,
            "target_pointer_width" => host_target_pointer_width() == val,
            _ => false,
        };
    }
    false
}

fn host_target_pointer_width() -> &'static str {
    #[cfg(target_pointer_width = "64")]
    return "64";
    #[cfg(target_pointer_width = "32")]
    return "32";
    #[cfg(not(any(target_pointer_width = "64", target_pointer_width = "32")))]
    return "";
}

fn host_target_os() -> &'static str {
    #[cfg(target_os = "linux")]
    return "linux";
    #[cfg(target_os = "macos")]
    return "macos";
    #[cfg(target_os = "windows")]
    return "windows";
    #[cfg(target_os = "freebsd")]
    return "freebsd";
    #[cfg(target_os = "openbsd")]
    return "openbsd";
    #[cfg(target_os = "netbsd")]
    return "netbsd";
    #[cfg(target_os = "dragonfly")]
    return "dragonfly";
    #[cfg(target_os = "android")]
    return "android";
    #[cfg(target_os = "ios")]
    return "ios";
    #[cfg(not(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "windows",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly",
        target_os = "android",
        target_os = "ios"
    )))]
    return "unknown";
}

fn host_target_arch() -> &'static str {
    #[cfg(target_arch = "x86_64")]
    return "x86_64";
    #[cfg(target_arch = "aarch64")]
    return "aarch64";
    #[cfg(target_arch = "arm")]
    return "arm";
    #[cfg(target_arch = "x86")]
    return "x86";
    #[cfg(target_arch = "riscv64")]
    return "riscv64";
    #[cfg(target_arch = "wasm32")]
    return "wasm32";
    #[cfg(not(any(
        target_arch = "x86_64",
        target_arch = "aarch64",
        target_arch = "arm",
        target_arch = "x86",
        target_arch = "riscv64",
        target_arch = "wasm32"
    )))]
    return "unknown";
}

fn host_target_env() -> &'static str {
    #[cfg(target_env = "gnu")]
    return "gnu";
    #[cfg(target_env = "musl")]
    return "musl";
    #[cfg(target_env = "msvc")]
    return "msvc";
    #[cfg(not(any(target_env = "gnu", target_env = "musl", target_env = "msvc")))]
    return "";
}

fn host_target_family() -> &'static str {
    #[cfg(unix)]
    return "unix";
    #[cfg(windows)]
    return "windows";
    #[cfg(not(any(unix, windows)))]
    return "";
}

fn host_target_vendor() -> &'static str {
    #[cfg(target_vendor = "apple")]
    return "apple";
    #[cfg(target_vendor = "pc")]
    return "pc";
    #[cfg(target_vendor = "unknown")]
    return "unknown";
    #[cfg(not(any(
        target_vendor = "apple",
        target_vendor = "pc",
        target_vendor = "unknown"
    )))]
    return "";
}
