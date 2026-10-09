/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{Context, Result};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

/// Discovers all applicable Cargo configuration files from global locations
/// and the filesystem root down to `start_dir`, ordered from lowest to highest precedence.
pub fn discover_cargo_config_files(start_dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut seen = HashSet::new();

    // 1. Global config in $CARGO_HOME or $HOME/.cargo (lowest precedence)
    let global_dir = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cargo")));

    if let Some(gdir) = global_dir {
        for name in &["config.toml", "config"] {
            let p = gdir.join(name);
            if p.is_file() {
                let canon = p.canonicalize().unwrap_or_else(|_| p.clone());
                if seen.insert(canon) {
                    files.push(p);
                }
            }
        }
    }

    // 2. Ancestors from root '/' down to start_dir (increasing precedence)
    let mut ancestor_chain = Vec::new();
    let mut curr = Some(start_dir);
    while let Some(d) = curr {
        ancestor_chain.push(d);
        curr = d.parent();
    }
    ancestor_chain.reverse();

    for dir in ancestor_chain {
        let cargo_dir = dir.join(".cargo");
        for name in &["config.toml", "config"] {
            let p = cargo_dir.join(name);
            if p.is_file() {
                let canon = p.canonicalize().unwrap_or_else(|_| p.clone());
                if seen.insert(canon) {
                    files.push(p);
                }
            }
        }
    }

    files
}

/// Discovers and parses applicable .cargo/config.toml or .cargo/config files,
/// merging rustflags defined in [target.<triple>].rustflags or [build].rustflags.
pub fn load_cargo_config_rustflags(
    package_dir: &Path,
    target_triple: Option<&str>,
) -> Result<Vec<String>> {
    let files = discover_cargo_config_files(package_dir);
    let mut merged = Vec::new();
    let host_triple = super::compiler_resolve::resolve_host_triple();
    let effective_target = target_triple.unwrap_or(host_triple);

    for config_path in files {
        let content = fs::read_to_string(&config_path)
            .with_context(|| format!("Failed to read cargo config at {:?}", config_path))?;
        let val: toml::Value = toml::from_str(&content)
            .with_context(|| format!("Failed to parse cargo config at {:?}", config_path))?;

        if let Some(flags) = extract_rustflags_from_toml(&val, effective_target, &config_path)? {
            merged.extend(flags);
        }
    }
    Ok(merged)
}

fn extract_rustflags_from_toml(
    val: &toml::Value,
    effective_target: &str,
    config_path: &Path,
) -> Result<Option<Vec<String>>> {
    if let Some(targets) = val.get("target").and_then(|t| t.as_table()) {
        // Fail-closed on target.'cfg(...)'.rustflags until arbitrary cfg expressions are supported
        for (key, target_val) in targets {
            if key.starts_with("cfg(") || key.contains("cfg(") {
                if let Some(target_table) = target_val.as_table() {
                    if target_table.contains_key("rustflags") {
                        anyhow::bail!(
                            "Unsupported cargo config: target.'cfg(...)' rustflags is not yet supported in {}",
                            config_path.display()
                        );
                    }
                }
            }
        }

        // 1. Check target.<effective_target>.rustflags
        if let Some(target_val) = targets.get(effective_target) {
            if let Some(rf) = target_val.get("rustflags") {
                return Ok(parse_flags_value(rf));
            }
        }
    }

    // 2. Check [build].rustflags
    if let Some(build_tbl) = val.get("build").and_then(|b| b.as_table()) {
        if let Some(rf) = build_tbl.get("rustflags") {
            return Ok(parse_flags_value(rf));
        }
    }

    Ok(None)
}

fn parse_flags_value(val: &toml::Value) -> Option<Vec<String>> {
    match val {
        toml::Value::String(s) => Some(s.split_whitespace().map(|x| x.to_string()).collect()),
        toml::Value::Array(arr) => {
            let mut res = Vec::new();
            for item in arr {
                if let toml::Value::String(s) = item {
                    res.push(s.clone());
                }
            }
            Some(res)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_extract_build_rustflags_string_and_array() {
        let content_str = r#"
[build]
rustflags = "-C opt-level=3 --cfg custom_mark"
"#;
        let val: toml::Value = toml::from_str(content_str).unwrap();
        let flags = extract_rustflags_from_toml(&val, "x86_64-unknown-linux-gnu", Path::new("dummy.toml"))
            .unwrap()
            .unwrap();
        assert_eq!(flags, vec!["-C", "opt-level=3", "--cfg", "custom_mark"]);

        let content_arr = r#"
[build]
rustflags = ["-C", "opt-level=2", "--cfg", "mark_array"]
"#;
        let val2: toml::Value = toml::from_str(content_arr).unwrap();
        let flags2 = extract_rustflags_from_toml(&val2, "x86_64-unknown-linux-gnu", Path::new("dummy.toml"))
            .unwrap()
            .unwrap();
        assert_eq!(flags2, vec!["-C", "opt-level=2", "--cfg", "mark_array"]);
    }

    #[test]
    fn test_extract_target_triple_rustflags_precedence() {
        let content = r#"
[build]
rustflags = ["--cfg", "build_flag"]

[target.x86_64-unknown-linux-gnu]
rustflags = ["--cfg", "linux_flag"]
"#;
        let val: toml::Value = toml::from_str(content).unwrap();
        let flags = extract_rustflags_from_toml(&val, "x86_64-unknown-linux-gnu", Path::new("dummy.toml"))
            .unwrap()
            .unwrap();
        assert_eq!(flags, vec!["--cfg", "linux_flag"]);

        let fallback_flags =
            extract_rustflags_from_toml(&val, "aarch64-apple-darwin", Path::new("dummy.toml"))
                .unwrap()
                .unwrap();
        assert_eq!(fallback_flags, vec!["--cfg", "build_flag"]);
    }

    #[test]
    fn test_extract_target_cfg_fails_closed() {
        let content = r#"
[target.'cfg(target_os = "linux")']
rustflags = ["--cfg", "linux_cfg"]
"#;
        let val: toml::Value = toml::from_str(content).unwrap();
        let res = extract_rustflags_from_toml(&val, "x86_64-unknown-linux-gnu", Path::new("custom.toml"));
        assert!(res.is_err());
        assert!(res.unwrap_err().to_string().contains("target.'cfg(...)' rustflags is not yet supported"));
    }

    #[test]
    fn test_load_cargo_config_rustflags_from_dir() {
        let dir = tempdir().unwrap();
        let cargo_dir = dir.path().join(".cargo");
        fs::create_dir_all(&cargo_dir).unwrap();
        fs::write(
            cargo_dir.join("config.toml"),
            "[build]\nrustflags = [\"--cfg\", \"test_loaded\"]\n",
        )
        .unwrap();

        let loaded = load_cargo_config_rustflags(dir.path(), None).unwrap();
        assert!(loaded.contains(&"--cfg".to_string()));
        assert!(loaded.contains(&"test_loaded".to_string()));
    }
}
