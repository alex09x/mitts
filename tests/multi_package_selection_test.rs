/*
 * Mitts Build — multiple package selection regression
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::process::Command;

#[test]
fn selected_root_union_keeps_both_dependency_closures() {
    use mitts::adapter::rust::{
        lockfile::{CargoLock, CargoLockPackage},
        manifest::CargoToml,
        package_selection::select_transitive_packages,
        types::RustBuildOptions,
    };
    let make = |name: &str, dependencies: Vec<String>| CargoLockPackage {
        name: name.into(),
        version: "1.0.0".into(),
        source: None,
        checksum: None,
        dependencies,
    };
    let lock = CargoLock {
        version: 4,
        packages: vec![
            make("a", vec!["left".into()]),
            make("b", vec!["right".into()]),
            make("left", vec![]),
            make("right", vec![]),
        ],
    };
    let options = RustBuildOptions {
        package: Some("a".into()),
        packages: vec!["b".into()],
        ..Default::default()
    };
    let selected = select_transitive_packages(
        &lock,
        &Default::default(),
        &CargoToml::default(),
        std::path::Path::new("."),
        &options,
    )
    .unwrap();
    let names: std::collections::HashSet<_> = selected
        .iter()
        .map(|(package, _)| package.name.as_str())
        .collect();
    assert_eq!(names, ["a", "b", "left", "right"].into_iter().collect());
}

#[test]
fn selected_packages_share_dependency_features() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("workspace");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers=['a','b','shared']\nresolver='2'\n",
    )
    .unwrap();
    for (name, feature) in [("a", "left"), ("b", "right")] {
        let dir = root.join(name);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("Cargo.toml"), format!("[package]\nname='{name}'\nversion='0.1.0'\nedition='2021'\n[features]\nown=[]\n[dependencies]\nshared={{path='../shared',features=['{feature}']}}\n")).unwrap();
        let gate = if name == "a" {
            "#[cfg(not(feature=\"own\"))] compile_error!(\"a needs own\");\n"
        } else {
            "#[cfg(feature=\"own\")] compile_error!(\"b must not get a's feature\");\n"
        };
        std::fs::write(
            dir.join("src/lib.rs"),
            format!("{gate}pub fn f() {{ shared::f(); }}\n"),
        )
        .unwrap();
    }
    std::fs::create_dir_all(root.join("shared/src")).unwrap();
    std::fs::write(root.join("shared/Cargo.toml"), "[package]\nname='shared'\nversion='0.1.0'\nedition='2021'\n[features]\nleft=[]\nright=[]\n").unwrap();
    std::fs::write(root.join("shared/src/lib.rs"), "#[cfg(not(all(feature=\"left\",feature=\"right\")))] compile_error!(\"features must be unified\");\npub fn f() {}\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_mitts"))
        .current_dir(&root)
        .args([
            "build",
            "--offline",
            "-p",
            "a",
            "-p",
            "b",
            "--features",
            "a/own",
            "--cache-dir",
        ])
        .arg(temp.path().join("cache"))
        .arg("--target-dir")
        .arg(temp.path().join("target"))
        .env_remove("CARGO_MAKEFLAGS")
        .env_remove("MAKEFLAGS")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let deps = temp.path().join("target/debug/deps");
    let files: Vec<_> = std::fs::read_dir(deps)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    for name in ["a", "b"] {
        assert!(
            files
                .iter()
                .any(|file| file.starts_with(&format!("lib{name}-")) && file.ends_with(".rlib")),
            "missing artifact for {name}: {files:?}"
        );
    }
    for command in ["check", "clippy", "test"] {
        let mut process = Command::new(env!("CARGO_BIN_EXE_mitts"));
        process
            .current_dir(&root)
            .args([
                command,
                "--offline",
                "-p",
                "a",
                "-p",
                "b",
                "--features",
                "a/own",
                "--cache-dir",
            ])
            .arg(temp.path().join("cache"))
            .arg("--target-dir")
            .arg(temp.path().join("target"))
            .env_remove("CARGO_MAKEFLAGS")
            .env_remove("MAKEFLAGS");
        if command == "test" {
            process.arg("--no-run");
        }
        let output = process.output().unwrap();
        assert!(
            output.status.success(),
            "{command}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let output = Command::new(env!("CARGO_BIN_EXE_mitts"))
        .current_dir(&root)
        .args([
            "explain",
            "-p",
            "a",
            "-p",
            "b",
            "--features",
            "a/own",
            "--json",
            "--cache-dir",
        ])
        .arg(temp.path().join("cache"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "explain: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let plan: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let units = plan["units"].as_array().unwrap();
    let shared = units
        .iter()
        .find(|unit| unit["package_name"] == "shared")
        .unwrap();
    let features = shared["features"].as_array().unwrap();
    assert!(features.contains(&"left".into()) && features.contains(&"right".into()));
    for (selectors, expected) in [
        (vec!["-p", "a", "-p", "missing"], "missing"),
        (
            vec!["-p", "a", "-p", "b", "--features", "unknown"],
            "unknown",
        ),
        (
            vec!["-p", "a", "-p", "b", "--features", "a/unknown"],
            "unknown",
        ),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_mitts"))
            .current_dir(&root)
            .args(["build", "--offline"])
            .args(selectors)
            .arg("--cache-dir")
            .arg(temp.path().join("cache"))
            .arg("--target-dir")
            .arg(temp.path().join("target"))
            .output()
            .unwrap();
        assert!(!output.status.success(), "invalid selection succeeded");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(expected),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
