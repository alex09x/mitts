/*
 * Mitts Build — a unit that fails after emitting metadata leaves nothing behind (issue 89)
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::Path;
use std::process::{Command, Output};

fn write(root: &Path, rel: &str, body: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

fn build(root: &Path, temp: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mitts"))
        .current_dir(root)
        .args(["build", "--offline", "-p", "top", "--cache-dir"])
        .arg(temp.join("cache"))
        .arg("--target-dir")
        .arg(temp.join("target"))
        .env_remove("MITTS_CACHE_DIR")
        .output()
        .unwrap()
}

fn package(name: &str, deps: &str) -> String {
    format!("[package]\nname='{name}'\nversion='0.1.0'\nedition='2021'\n[dependencies]\n{deps}")
}

/// Action-cache entries whose unit id ends in `:<crate>` (any kind).
fn entries_for(cache: &Path, crate_name: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut stack = vec![cache.join("ac")];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in rd.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|e| e.to_str()) == Some("json") {
                let text = std::fs::read_to_string(&path).unwrap_or_default();
                let json: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
                let id = json["action_id"].as_str().unwrap_or_default();
                if id.ends_with(&format!(":{crate_name}")) {
                    found.push(id.to_string());
                }
            }
        }
    }
    found
}

/// A library whose metadata is fine but whose codegen fails: the assembler rejects the
/// instruction only when LLVM emits the function, after rustc has written the `.rmeta`.
/// Heavy functions ahead of it give metadata-only dependents time to start and finish.
fn bad_lib(broken: bool) -> String {
    let mut src = String::new();
    for i in 0..3000 {
        src.push_str(&format!(
            "pub fn filler_{i}(v: &mut Vec<u64>) -> u64 {{ let mut a = {i}u64; \
             for x in v.iter_mut() {{ *x = x.wrapping_mul(a).rotate_left(7); a ^= *x; }} \
             v.sort(); a }}\n"
        ));
    }
    let insn = if broken {
        "definitely_not_an_instruction"
    } else {
        "nop"
    };
    src.push_str(&format!(
        "pub fn trigger() {{ unsafe {{ core::arch::asm!(\"{insn}\") }} }}\n"
    ));
    src
}

#[test]
fn a_codegen_failure_after_metadata_publishes_nothing_for_the_unit_or_its_dependents() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("ws");
    write(
        &root,
        "Cargo.toml",
        "[workspace]\nresolver='2'\nmembers=['bad','mid','top']\n",
    );
    write(&root, "bad/Cargo.toml", &package("bad", ""));
    write(&root, "bad/src/lib.rs", &bad_lib(true));
    write(
        &root,
        "mid/Cargo.toml",
        &package("mid", "bad={path='../bad'}\n"),
    );
    write(&root, "mid/src/lib.rs", "pub fn m() { bad::trigger(); }\n");
    write(
        &root,
        "top/Cargo.toml",
        &package("top", "mid={path='../mid'}\n"),
    );
    write(&root, "top/src/lib.rs", "pub fn t() { mid::m(); }\n");

    let failed = build(&root, temp.path());
    assert!(
        !failed.status.success(),
        "the post-monomorphization error must fail the build"
    );
    let cache = temp.path().join("cache");
    for name in ["bad", "mid", "top"] {
        assert!(
            entries_for(&cache, name).is_empty(),
            "no action-cache entry may survive for {name}: {:?}",
            entries_for(&cache, name)
        );
    }
    let deps = temp.path().join("target/debug/deps");
    let leftover: Vec<_> = std::fs::read_dir(&deps)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.starts_with("libbad"))
                .collect()
        })
        .unwrap_or_default();
    assert!(
        leftover.is_empty(),
        "failed unit's metadata left in target/: {leftover:?}"
    );

    // Fixed, the same cache and target dir build correctly.
    write(&root, "bad/src/lib.rs", &bad_lib(false));
    let fixed = build(&root, temp.path());
    assert!(
        fixed.status.success(),
        "build after the fix failed:\n{}",
        String::from_utf8_lossy(&fixed.stderr)
    );
}
