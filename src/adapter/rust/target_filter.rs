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
    let clean = target_spec.trim().trim_matches('\'').trim_matches('"').trim();

    // Direct triple matching (e.g. "x86_64-pc-windows-msvc", "wasm32-unknown-unknown")
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

    // Target env
    #[cfg(not(target_env = "musl"))]
    if clean.contains("target_env = \"musl\"") || clean.contains("target_env=\"musl\"") {
        return true;
    }
    #[cfg(not(target_env = "msvc"))]
    if clean.contains("target_env = \"msvc\"") || clean.contains("target_env=\"msvc\"") {
        return true;
    }
    #[cfg(not(target_env = "gnu"))]
    if clean.contains("target_env = \"gnu\"") || clean.contains("target_env=\"gnu\"") {
        return true;
    }

    // Target OS
    #[cfg(not(target_os = "windows"))]
    if clean.contains("target_os = \"windows\"")
        || clean.contains("target_os=\"windows\"")
        || clean == "cfg(windows)"
    {
        return true;
    }
    #[cfg(not(target_os = "android"))]
    if clean.contains("target_os = \"android\"") || clean.contains("target_os=\"android\"") {
        return true;
    }
    #[cfg(not(target_os = "redox"))]
    if clean.contains("target_os = \"redox\"") || clean.contains("target_os=\"redox\"") {
        return true;
    }
    #[cfg(not(target_os = "ios"))]
    if clean.contains("target_os = \"ios\"") || clean.contains("target_os=\"ios\"") {
        return true;
    }
    #[cfg(not(any(target_os = "macos", target_os = "ios")))]
    if clean.contains("target_os = \"macos\"") || clean.contains("target_os=\"macos\"") {
        return true;
    }
    #[cfg(not(any(
        target_os = "linux",
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd"
    )))]
    if clean.contains("target_os = \"linux\"") || clean.contains("target_os=\"linux\"") {
        return true;
    }

    // Target family
    #[cfg(not(unix))]
    if clean == "cfg(unix)" || clean.contains("target_family = \"unix\"") {
        return true;
    }
    #[cfg(not(windows))]
    if clean.contains("target_family = \"windows\"") {
        return true;
    }

    // Target arch
    #[cfg(not(any(target_arch = "wasm32", target_arch = "wasm64")))]
    if clean.contains("target_arch = \"wasm32\"")
        || clean.contains("target_arch=\"wasm32\"")
        || clean.contains("target_arch = \"wasm64\"")
    {
        return true;
    }

    false
}
