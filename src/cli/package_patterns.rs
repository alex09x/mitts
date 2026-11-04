/*
 * Mitts Build — package file pattern selection
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{bail, Context, Result};
use std::collections::HashSet;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

/// Evaluate rules independently so later negations can override parent matches.
pub fn select(paths: &[String], rules: &[String], include: bool) -> Result<HashSet<String>> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let dir = std::env::temp_dir().join(format!(
        "mitts-patterns-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir(&dir)?;
    let _cleanup = Cleanup(dir.clone());
    let status = Command::new("git")
        .args(["init", "-q"])
        .current_dir(&dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_CONFIG_COUNT")
        .env_remove("GIT_CONFIG_PARAMETERS")
        .env_remove("GIT_TEMPLATE_DIR")
        .status()?;
    if !status.success() {
        bail!("failed to initialize pattern matcher");
    }
    let mut input = Vec::new();
    for (index, rule) in rules.iter().enumerate() {
        if rule.contains(['\n', '\r', '\0']) {
            bail!("invalid package pattern");
        }
        let pattern = rule.strip_prefix('!').unwrap_or(rule);
        let subdir = dir.join(format!("rule-{index}"));
        std::fs::create_dir(&subdir)?;
        std::fs::write(subdir.join(".gitignore"), format!("{pattern}\n"))?;
        for path in paths {
            input.extend_from_slice(format!("rule-{index}/{path}").as_bytes());
            input.push(0);
        }
    }
    if rules.is_empty() {
        return Ok(if include {
            HashSet::new()
        } else {
            paths.iter().cloned().collect()
        });
    }
    let mut child = Command::new("git")
        .current_dir(&dir)
        .args([
            "-c",
            "core.excludesFile=/dev/null",
            "check-ignore",
            "--no-index",
            "--verbose",
            "--non-matching",
            "-z",
            "--stdin",
        ])
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_CONFIG_COUNT")
        .env_remove("GIT_CONFIG_PARAMETERS")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut stdin = child
        .stdin
        .take()
        .context("missing pattern matcher stdin")?;
    let writer = std::thread::spawn(move || stdin.write_all(&input));
    let output = child.wait_with_output()?;
    writer
        .join()
        .map_err(|_| anyhow::anyhow!("pattern matcher input failed"))??;
    if !matches!(output.status.code(), Some(0 | 1)) {
        bail!("pattern matcher failed");
    }
    let fields: Vec<&[u8]> = output.stdout.split(|b| *b == 0).collect();
    if fields.len() != rules.len() * paths.len() * 4 + 1 {
        bail!("invalid pattern matcher output");
    }
    let mut selected: HashSet<String> = if include {
        HashSet::new()
    } else {
        paths.iter().cloned().collect()
    };
    for (index, rule) in rules.iter().enumerate() {
        let negative = rule.starts_with('!');
        for (path_index, path) in paths.iter().enumerate() {
            if !fields[(index * paths.len() + path_index) * 4].is_empty() {
                if include != negative {
                    selected.insert(path.clone());
                } else {
                    selected.remove(path);
                }
            }
        }
    }
    Ok(selected)
}

#[cfg(test)]
mod tests {
    use super::select;

    #[test]
    fn ordered_include_and_exclude_patterns_match_nested_files() {
        let paths: Vec<String> = [
            "src/a.rs",
            "src/secret.rs",
            "docs/note1.txt",
            "unrelated.txt",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        let include = select(
            &paths,
            &[
                "*.rs".into(),
                "!src/secret.rs".into(),
                "**/note?.txt".into(),
            ],
            true,
        )
        .unwrap();
        assert!(include.contains("src/a.rs"));
        assert!(!include.contains("src/secret.rs"));
        assert!(include.contains("docs/note1.txt"));
        assert!(!include.contains("unrelated.txt"));
        let exclude = select(&paths, &["src/".into(), "!src/a.rs".into()], false).unwrap();
        assert!(exclude.contains("src/a.rs"));
        assert!(!exclude.contains("src/secret.rs"));
        assert!(exclude.contains("unrelated.txt"));
    }
}
