/*
 * Mitts Build — package archive tests
 * Copyright (c) 2026 Alexander Panasenko
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */
use super::create;
use std::io::Read;
use std::process::Command;

#[test]
fn include_patterns_honor_nested_negation() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    std::fs::create_dir(root.join("src")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname='demo'\nversion='1.0.0'\ninclude=['/src', '!/src/secret.rs']\n",
    )
    .unwrap();
    std::fs::write(
        root.join("Cargo.lock"),
        "version=4\n[[package]]\nname='demo'\nversion='1.0.0'\n",
    )
    .unwrap();
    std::fs::write(root.join("src/lib.rs"), "pub fn f() {}\n").unwrap();
    std::fs::write(root.join("src/secret.rs"), "not for distribution\n").unwrap();
    std::fs::write(root.join(".gitignore"), "src/ignored.rs\n").unwrap();
    std::fs::write(root.join("src/ignored.rs"), "explicitly included\n").unwrap();
    std::fs::write(root.join("notes.txt"), "not included\n").unwrap();
    for args in [
        vec!["init", "-q"],
        vec!["add", "."],
        vec![
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-qm",
            "fixture",
        ],
    ] {
        assert!(Command::new("git")
            .current_dir(root)
            .args(args)
            .status()
            .unwrap()
            .success());
    }
    let bytes = create(root, "demo", "1.0.0").unwrap();
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(bytes.as_slice()));
    let paths: Vec<_> = archive
        .entries()
        .unwrap()
        .map(|entry| entry.unwrap().path().unwrap().to_path_buf())
        .collect();
    assert!(paths.contains(&"demo-1.0.0/src/lib.rs".into()));
    assert!(paths.contains(&"demo-1.0.0/src/ignored.rs".into()));
    assert!(!paths.contains(&"demo-1.0.0/src/secret.rs".into()));
    assert!(!paths.contains(&"demo-1.0.0/notes.txt".into()));
    assert!(paths.contains(&"demo-1.0.0/Cargo.toml".into()));
    assert!(paths.contains(&"demo-1.0.0/Cargo.lock".into()));
}

#[test]
fn included_package_always_contains_ignored_license_file() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    std::fs::create_dir(root.join("src")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname='demo'\nversion='1.0.0'\ninclude=['/src']\nlicense-file='LICENSE'\n",
    )
    .unwrap();
    std::fs::write(
        root.join("Cargo.lock"),
        "version=4\n[[package]]\nname='demo'\nversion='1.0.0'\n",
    )
    .unwrap();
    std::fs::write(root.join(".gitignore"), "LICENSE\n").unwrap();
    std::fs::write(root.join("LICENSE"), "license text\n").unwrap();
    std::fs::write(root.join("src/lib.rs"), "pub fn f() {}\n").unwrap();
    for args in [
        vec!["init", "-q"],
        vec!["add", "."],
        vec![
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-qm",
            "fixture",
        ],
    ] {
        assert!(Command::new("git")
            .current_dir(root)
            .args(args)
            .status()
            .unwrap()
            .success());
    }
    let bytes = create(root, "demo", "1.0.0").unwrap();
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(bytes.as_slice()));
    let paths: Vec<_> = archive
        .entries()
        .unwrap()
        .map(|entry| entry.unwrap().path().unwrap().to_path_buf())
        .collect();
    assert!(paths.contains(&"demo-1.0.0/LICENSE".into()));
}

#[test]
fn includes_tracked_file_omitted_by_git_export_ignore() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname='demo'\nversion='1.0.0'\n",
    )
    .unwrap();
    std::fs::write(
        root.join("Cargo.lock"),
        "version=4\n[[package]]\nname='demo'\nversion='1.0.0'\n",
    )
    .unwrap();
    std::fs::write(root.join(".gitattributes"), "hidden.rs export-ignore\nhidden_link.rs export-ignore\nCargo.toml export-ignore\nCargo.lock export-ignore\n").unwrap();
    std::fs::write(root.join("hidden.rs"), "pub fn f() {}\n").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink("hidden.rs", root.join("hidden_link.rs")).unwrap();
    for args in [
        vec!["init", "-q"],
        vec!["add", "."],
        vec![
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-qm",
            "fixture",
        ],
    ] {
        assert!(Command::new("git")
            .current_dir(root)
            .args(args)
            .status()
            .unwrap()
            .success());
    }
    let data = create(root, "demo", "1.0.0").unwrap();
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(data.as_slice()));
    let mut found = false;
    let mut paths = std::collections::HashSet::new();
    for entry in archive.entries().unwrap() {
        let mut entry = entry.unwrap();
        paths.insert(entry.path().unwrap().to_path_buf());
        if entry.path().unwrap() == std::path::Path::new("demo-1.0.0/hidden.rs")
            || entry.path().unwrap() == std::path::Path::new("demo-1.0.0/hidden_link.rs")
        {
            let mut text = String::new();
            entry.read_to_string(&mut text).unwrap();
            assert_eq!(text, "pub fn f() {}\n");
            found = true;
        }
    }
    assert!(found);
    assert!(paths.contains(std::path::Path::new("demo-1.0.0/hidden.rs")));
    assert!(paths.contains(std::path::Path::new("demo-1.0.0/Cargo.toml")));
    assert!(paths.contains(std::path::Path::new("demo-1.0.0/Cargo.toml.orig")));
    assert!(paths.contains(std::path::Path::new("demo-1.0.0/Cargo.lock")));
    #[cfg(unix)]
    assert!(paths.contains(std::path::Path::new("demo-1.0.0/hidden_link.rs")));
}

#[test]
fn packages_committed_source_and_refuses_dirty_checkout() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname='demo'\nversion='1.0.0'\n",
    )
    .unwrap();
    std::fs::write(
        root.join("Cargo.lock"),
        "version = 4\n[[package]]\nname='demo'\nversion='1.0.0'\n",
    )
    .unwrap();
    std::fs::write(root.join("lib.rs"), "pub fn demo() {}\n").unwrap();
    std::fs::create_dir(root.join("nested")).unwrap();
    std::fs::write(
        root.join("nested/Cargo.toml"),
        "[package]\nname='nested'\nversion='1.0.0'\n",
    )
    .unwrap();
    std::fs::create_dir(root.join("target")).unwrap();
    std::fs::write(root.join("target/ignored"), "not packaged").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink("lib.rs", root.join("link.rs")).unwrap();
    for args in [
        vec!["init", "-q"],
        vec!["add", "."],
        vec![
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-qm",
            "fixture",
        ],
    ] {
        assert!(Command::new("git")
            .current_dir(root)
            .args(args)
            .status()
            .unwrap()
            .success());
    }
    let sha = Command::new("git")
        .current_dir(root)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    let sha = String::from_utf8(sha.stdout).unwrap();
    let data = create(root, "demo", "1.0.0").unwrap();
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(data.as_slice()));
    let mut files = std::collections::BTreeMap::new();
    for entry in archive.entries().unwrap() {
        let mut entry = entry.unwrap();
        let path = entry.path().unwrap().to_string_lossy().into_owned();
        let mut contents = String::new();
        entry.read_to_string(&mut contents).unwrap();
        files.insert(path, contents);
    }
    assert!(files.contains_key("demo-1.0.0/Cargo.toml.orig"));
    assert_eq!(files["demo-1.0.0/lib.rs"], "pub fn demo() {}\n");
    assert!(files
        .keys()
        .all(|name| !name.starts_with("demo-1.0.0/nested/")
            && !name.starts_with("demo-1.0.0/target/")));
    #[cfg(unix)]
    assert_eq!(files["demo-1.0.0/link.rs"], "pub fn demo() {}\n");
    let vcs: serde_json::Value =
        serde_json::from_str(&files["demo-1.0.0/.cargo_vcs_info.json"]).unwrap();
    assert_eq!(vcs["git"]["sha1"], sha.trim());
    std::fs::write(root.join("lib.rs"), "changed\n").unwrap();
    assert!(create(root, "demo", "1.0.0").is_err());
}
