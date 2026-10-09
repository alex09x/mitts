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

/// Finds either bare `config` (preferred by Cargo) or `config.toml` in `dir`.
fn find_config_in_dir(dir: &Path) -> Option<PathBuf> {
    for name in &["config", "config.toml"] {
        let p = dir.join(name);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

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
        if let Some(p) = find_config_in_dir(&gdir) {
            let canon = p.canonicalize().unwrap_or_else(|_| p.clone());
            if seen.insert(canon) {
                files.push(p);
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
        if let Some(p) = find_config_in_dir(&cargo_dir) {
            let canon = p.canonicalize().unwrap_or_else(|_| p.clone());
            if seen.insert(canon) {
                files.push(p);
            }
        }
    }

    files
}

/// Discovers and parses applicable .cargo/config.toml or .cargo/config files,
/// merging rustflags according to Cargo's precedence rules:
/// 1. CARGO_TARGET_<UPPER_TRIPLE>_RUSTFLAGS environment variable
/// 2. target.<triple>.rustflags merged across config files
/// 3. CARGO_BUILD_RUSTFLAGS environment variable
/// 4. build.rustflags merged across config files
pub fn load_cargo_config_rustflags(
    start_dir: &Path,
    target_triple: Option<&str>,
) -> Result<Vec<String>> {
    let host_triple = super::compiler_resolve::resolve_host_triple();
    let effective_target = target_triple.unwrap_or(host_triple);

    // 1. Discover and parse cargo config files
    let files = discover_cargo_config_files(start_dir);
    let mut all_target_flags = Vec::new();
    let mut all_build_flags = Vec::new();

    for config_path in files {
        let content = fs::read_to_string(&config_path)
            .with_context(|| format!("Failed to read cargo config at {:?}", config_path))?;
        let val: toml::Value = toml::from_str(&content)
            .with_context(|| format!("Failed to parse cargo config at {:?}", config_path))?;

        let extracted = extract_rustflags_from_toml(&val, effective_target, &config_path)?;
        if let Some(tf) = extracted.target {
            all_target_flags.extend(tf);
        }
        if let Some(bf) = extracted.build {
            all_build_flags.extend(bf);
        }
    }

    // 2. CARGO_TARGET_<UPPER_TRIPLE>_RUSTFLAGS environment variable (appended to config file flags)
    let upper_triple = effective_target.replace('-', "_").to_uppercase();
    let target_env_key = format!("CARGO_TARGET_{}_RUSTFLAGS", upper_triple);
    if let Ok(flags_str) = std::env::var(&target_env_key) {
        all_target_flags.extend(flags_str.split_whitespace().map(|s| s.to_string()));
    }

    // 3. If any target.<triple>.rustflags was found in configs or env, it takes precedence
    if !all_target_flags.is_empty() {
        return Ok(all_target_flags);
    }

    // 4. CARGO_BUILD_RUSTFLAGS environment variable (appended to config file flags)
    if let Ok(flags_str) = std::env::var("CARGO_BUILD_RUSTFLAGS") {
        all_build_flags.extend(flags_str.split_whitespace().map(|s| s.to_string()));
    }

    // 5. Merged [build].rustflags across all config files and env
    if !all_build_flags.is_empty() {
        return Ok(all_build_flags);
    }

    Ok(Vec::new())
}

#[derive(Default, Debug, PartialEq, Eq)]
struct ExtractedRustflags {
    target: Option<Vec<String>>,
    build: Option<Vec<String>>,
}

fn extract_rustflags_from_toml(
    val: &toml::Value,
    effective_target: &str,
    config_path: &Path,
) -> Result<ExtractedRustflags> {
    let mut extracted = ExtractedRustflags::default();

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
                extracted.target = parse_flags_value(rf);
            }
        }
    }

    // 2. Check [build].rustflags
    if let Some(build_tbl) = val.get("build").and_then(|b| b.as_table()) {
        if let Some(rf) = build_tbl.get("rustflags") {
            extracted.build = parse_flags_value(rf);
        }
    }

    Ok(extracted)
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

/// Resolves the unified Cargo config start directory given a base project or workspace directory.
/// If `explicit_start_dir` is provided, returns that directory.
/// Otherwise, if `current_dir()` is within `base_dir` or `base_dir` is within `current_dir()`,
/// returns `current_dir()` (e.g. member subdirectory during invocation);
/// otherwise falls back to `base_dir` (e.g. in isolated test environments).
pub fn resolve_config_start_dir_with_opt(
    base_dir: &Path,
    explicit_start_dir: Option<&Path>,
) -> PathBuf {
    if let Some(explicit) = explicit_start_dir {
        return explicit.to_path_buf();
    }
    if let Ok(cwd) = std::env::current_dir() {
        if cwd.starts_with(base_dir) || base_dir.starts_with(&cwd) {
            return cwd;
        }
    }
    base_dir.to_path_buf()
}

pub fn resolve_config_start_dir(base_dir: &Path) -> PathBuf {
    resolve_config_start_dir_with_opt(base_dir, None)
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
        let extracted = extract_rustflags_from_toml(&val, "x86_64-unknown-linux-gnu", Path::new("dummy.toml"))
            .unwrap();
        assert_eq!(extracted.build.unwrap(), vec!["-C", "opt-level=3", "--cfg", "custom_mark"]);

        let content_arr = r#"
[build]
rustflags = ["-C", "opt-level=2", "--cfg", "mark_array"]
"#;
        let val2: toml::Value = toml::from_str(content_arr).unwrap();
        let extracted2 = extract_rustflags_from_toml(&val2, "x86_64-unknown-linux-gnu", Path::new("dummy.toml"))
            .unwrap();
        assert_eq!(extracted2.build.unwrap(), vec!["-C", "opt-level=2", "--cfg", "mark_array"]);
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
        let extracted = extract_rustflags_from_toml(&val, "x86_64-unknown-linux-gnu", Path::new("dummy.toml"))
            .unwrap();
        assert_eq!(extracted.target.unwrap(), vec!["--cfg", "linux_flag"]);
        assert_eq!(extracted.build.unwrap(), vec!["--cfg", "build_flag"]);
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

    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct EnvGuard {
        cargo_home_orig: Option<std::ffi::OsString>,
    }

    impl EnvGuard {
        fn new(isolated_cargo_home: &Path) -> Self {
            let cargo_home_orig = std::env::var_os("CARGO_HOME");
            std::env::set_var("CARGO_HOME", isolated_cargo_home);
            Self { cargo_home_orig }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            if let Some(ref orig) = self.cargo_home_orig {
                std::env::set_var("CARGO_HOME", orig);
            } else {
                std::env::remove_var("CARGO_HOME");
            }
        }
    }

    #[test]
    fn test_load_cargo_config_rustflags_from_dir() {
        let _lock = TEST_LOCK.lock().unwrap();
        let home = tempdir().unwrap();
        let _guard = EnvGuard::new(home.path());

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

    #[test]
    fn test_bare_config_precedence_over_config_toml() {
        let _lock = TEST_LOCK.lock().unwrap();
        let home = tempdir().unwrap();
        let _guard = EnvGuard::new(home.path());

        let dir = tempdir().unwrap();
        let cargo_dir = dir.path().join(".cargo");
        fs::create_dir_all(&cargo_dir).unwrap();
        fs::write(
            cargo_dir.join("config"),
            "[build]\nrustflags = [\"--cfg\", \"from_bare_config\"]\n",
        )
        .unwrap();
        fs::write(
            cargo_dir.join("config.toml"),
            "[build]\nrustflags = [\"--cfg\", \"from_config_toml\"]\n",
        )
        .unwrap();

        let files = discover_cargo_config_files(dir.path());
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].file_name().unwrap(), "config");

        let loaded = load_cargo_config_rustflags(dir.path(), None).unwrap();
        assert_eq!(loaded, vec!["--cfg", "from_bare_config"]);
    }

    #[test]
    fn test_target_overrides_build_across_files() {
        let _lock = TEST_LOCK.lock().unwrap();
        let home = tempdir().unwrap();
        let _guard = EnvGuard::new(home.path());

        let parent = tempdir().unwrap();
        let child = parent.path().join("child");
        fs::create_dir_all(&child).unwrap();

        let parent_cargo = parent.path().join(".cargo");
        fs::create_dir_all(&parent_cargo).unwrap();
        fs::write(
            parent_cargo.join("config.toml"),
            "[build]\nrustflags = [\"--cfg\", \"parent_build\"]\n",
        )
        .unwrap();

        let child_cargo = child.join(".cargo");
        fs::create_dir_all(&child_cargo).unwrap();
        fs::write(
            child_cargo.join("config.toml"),
            "[target.x86_64-unknown-linux-gnu]\nrustflags = [\"--cfg\", \"child_target\"]\n",
        )
        .unwrap();

        let loaded = load_cargo_config_rustflags(&child, Some("x86_64-unknown-linux-gnu")).unwrap();
        assert_eq!(loaded, vec!["--cfg", "child_target"]);
    }
}
