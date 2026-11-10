/*
 * Mitts Build — declared extra outputs of a cached rustc wrapper (issue 91)
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

#![cfg(unix)]

use std::path::Path;
use std::process::{Command, Output};

fn write(root: &Path, rel: &str, body: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

/// An indexer-like wrapper: per compiled crate it writes `<crate>.idx` and a sidecar
/// `.deps/<crate>.json` into the directory Mitts names, and logs the crate it ran for.
const WRAPPER: &str = r#"#!/bin/sh
rustc="$1"; shift
name=""; prev=""
for a in "$@"; do
  if [ "$prev" = "--crate-name" ]; then name="$a"; fi
  prev="$a"
done
if [ -n "$name" ] && [ -n "$MITTS_WRAPPER_OUT_DIR" ]; then
  echo "index of $name" > "$MITTS_WRAPPER_OUT_DIR/$name.idx"
  mkdir -p "$MITTS_WRAPPER_OUT_DIR/.deps"
  echo "{\"crate\":\"$name\"}" > "$MITTS_WRAPPER_OUT_DIR/.deps/$name.json"
  echo "$name" >> "$WRAPPER_LOG"
fi
exec "$rustc" "$@"
"#;

fn build(root: &Path, temp: &Path, target: &str, salt: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mitts"))
        .current_dir(root)
        .args(["build", "--offline", "--cache-dir"])
        .arg(temp.join("cache"))
        .arg("--target-dir")
        .arg(temp.join(target))
        .env_remove("MITTS_CACHE_DIR")
        .env("RUSTC_WRAPPER", temp.join("wrapper.sh"))
        .env("MITTS_RUSTC_WRAPPER_CACHE", "1")
        .env("MITTS_RUSTC_WRAPPER_SALT", salt)
        .env("MITTS_RUSTC_WRAPPER_OUTPUTS", temp.join("index"))
        .env("WRAPPER_LOG", temp.join("wrapper.log"))
        .output()
        .unwrap()
}

fn runs(temp: &Path) -> usize {
    std::fs::read_to_string(temp.join("wrapper.log"))
        .map(|s| s.lines().count())
        .unwrap_or(0)
}

#[test]
fn a_wrapper_cache_hit_restores_the_wrappers_declared_outputs() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let wrapper = temp.path().join("wrapper.sh");
    std::fs::write(&wrapper, WRAPPER).unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let root = temp.path().join("ws");
    write(
        &root,
        "Cargo.toml",
        "[workspace]\nresolver='2'\nmembers=['dep','app']\n",
    );
    write(
        &root,
        "dep/Cargo.toml",
        "[package]\nname='dep'\nversion='0.1.0'\nedition='2021'\n",
    );
    write(&root, "dep/src/lib.rs", "pub fn d() {}\n");
    write(
        &root,
        "app/Cargo.toml",
        "[package]\nname='app'\nversion='0.1.0'\nedition='2021'\n[dependencies]\ndep={path='../dep'}\n",
    );
    write(&root, "app/src/lib.rs", "pub fn a() { dep::d() }\n");
    let index = temp.path().join("index");

    let first = build(&root, temp.path(), "t1", "s1");
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let ran = runs(temp.path());
    assert!(ran >= 2, "the wrapper must run for both crates, ran {ran}");
    for name in ["dep", "app"] {
        assert!(index.join(format!("{name}.idx")).is_file(), "{name}.idx");
        assert!(index.join(format!(".deps/{name}.json")).is_file());
    }

    // The outputs are gone; a hit must bring them back without running the wrapper.
    std::fs::remove_dir_all(&index).unwrap();
    let second = build(&root, temp.path(), "t2", "s1");
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    assert_eq!(
        runs(temp.path()),
        ran,
        "a cache hit must not run the wrapper"
    );
    for name in ["dep", "app"] {
        assert_eq!(
            std::fs::read_to_string(index.join(format!("{name}.idx"))).unwrap(),
            format!("index of {name}\n")
        );
        assert!(index.join(format!(".deps/{name}.json")).is_file());
    }

    // Another salt is another wrapper state: it misses and the wrapper runs again.
    let third = build(&root, temp.path(), "t3", "s2");
    assert!(
        third.status.success(),
        "{}",
        String::from_utf8_lossy(&third.stderr)
    );
    assert!(runs(temp.path()) > ran, "a changed salt must miss");
}
