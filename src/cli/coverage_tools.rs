/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::PathBuf;
use std::process::Command;

/// Resolves standard LLVM tools (llvm-profdata, llvm-cov) from the rustlib sysroot or PATH.
pub fn resolve_llvm_tool(tool_name: &str) -> Option<PathBuf> {
    let env_var = match tool_name {
        "llvm-profdata" => "MITTS_LLVM_PROFDATA",
        "llvm-cov" => "MITTS_LLVM_COV",
        _ => "",
    };
    if !env_var.is_empty() {
        if let Ok(val) = std::env::var(env_var) {
            if val == "missing" || val == "disabled" || val == "none" {
                return None;
            }
            if !val.is_empty() {
                let p = PathBuf::from(val);
                if p.is_file() {
                    return Some(p);
                }
                return None;
            }
        }
    }
    if let Ok(output) = Command::new("rustc").arg("--print").arg("sysroot").output() {
        if output.status.success() {
            let sysroot = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if let Ok(host_out) = Command::new("rustc").arg("-vV").output() {
                let host_str = String::from_utf8_lossy(&host_out.stdout);
                for line in host_str.lines() {
                    if let Some(host) = line.strip_prefix("host: ") {
                        let tool_path = PathBuf::from(&sysroot)
                            .join("lib")
                            .join("rustlib")
                            .join(host.trim())
                            .join("bin")
                            .join(tool_name);
                        if tool_path.is_file() {
                            return Some(tool_path);
                        }
                    }
                }
            }
        }
    }
    if let Some(paths) = std::env::var_os("PATH") {
        for path in std::env::split_paths(&paths) {
            let candidate = path.join(tool_name);
            if candidate.is_file() {
                return Some(candidate);
            }
            #[cfg(windows)]
            {
                let candidate_exe = path.join(format!("{}.exe", tool_name));
                if candidate_exe.is_file() {
                    return Some(candidate_exe);
                }
            }
        }
    }
    None
}
