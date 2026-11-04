/*
 * Mitts Build — native job limit regression
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::process::Command;

#[test]
fn jobs_limit_build_scripts_and_reject_invalid_values() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    let state = temp.path().join("state");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir(&state).unwrap();
    std::fs::write(root.join("Cargo.toml"), "[package]\nname='root'\nversion='0.1.0'\nedition='2021'\n[dependencies]\na={path='a'}\nb={path='b'}\n").unwrap();
    std::fs::write(root.join("src/lib.rs"), "pub fn f() { a::f(); b::f(); }\n").unwrap();
    let script = r#"
        fn main() {
            println!("cargo:rerun-if-env-changed=SHARED_STATE");
            let state = std::path::PathBuf::from(std::env::var_os("SHARED_STATE").unwrap());
            let lock = state.join("lock");
            let acquire = || { while std::fs::create_dir(&lock).is_err() { std::thread::sleep(std::time::Duration::from_millis(2)); } };
            acquire();
            let running: usize = std::fs::read_to_string(state.join("running")).unwrap_or_default().parse().unwrap_or(0);
            let maximum: usize = std::fs::read_to_string(state.join("max")).unwrap_or_default().parse().unwrap_or(0);
            std::fs::write(state.join("running"), (running + 1).to_string()).unwrap();
            std::fs::write(state.join("max"), maximum.max(running + 1).to_string()).unwrap();
            std::fs::write(state.join(format!("jobs-{}", std::env::var("CARGO_PKG_NAME").unwrap())), std::env::var("NUM_JOBS").unwrap()).unwrap();
            std::fs::remove_dir(&lock).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(150));
            acquire();
            let running: usize = std::fs::read_to_string(state.join("running")).unwrap().parse().unwrap();
            std::fs::write(state.join("running"), (running - 1).to_string()).unwrap();
            std::fs::remove_dir(&lock).unwrap();
        }
    "#;
    for name in ["a", "b"] {
        let dir = root.join(name);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(
            dir.join("Cargo.toml"),
            format!("[package]\nname='{name}'\nversion='0.1.0'\nedition='2021'\n"),
        )
        .unwrap();
        std::fs::write(dir.join("src/lib.rs"), "pub fn f() {}\n").unwrap();
        std::fs::write(dir.join("build.rs"), script).unwrap();
    }
    let run = |jobs: &str| {
        Command::new(env!("CARGO_BIN_EXE_mitts"))
            .current_dir(&root)
            .args(["build", "--offline", "--cache-dir"])
            .arg(temp.path().join("cache"))
            .arg("--target-dir")
            .arg(temp.path().join("target"))
            .env("MITTS_JOBS", jobs)
            .env("MITTS_FAST_LOCK", "1")
            .env("SHARED_STATE", &state)
            .env_remove("CARGO_MAKEFLAGS")
            .env_remove("MAKEFLAGS")
            .output()
            .unwrap()
    };
    let invalid = run("0");
    assert!(!invalid.status.success());
    assert!(
        String::from_utf8_lossy(&invalid.stderr).contains("MITTS_JOBS must be a positive integer")
    );
    let result = run("1");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(std::fs::read_to_string(state.join("max")).unwrap(), "1");
    for name in ["a", "b"] {
        assert_eq!(
            std::fs::read_to_string(state.join(format!("jobs-{name}"))).unwrap(),
            "1"
        );
    }
    std::fs::write(state.join("max"), "0").unwrap();
    let result = run("2");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let maximum: usize = std::fs::read_to_string(state.join("max"))
        .unwrap()
        .parse()
        .unwrap();
    assert!((1..=2).contains(&maximum));
    for name in ["a", "b"] {
        assert_eq!(
            std::fs::read_to_string(state.join(format!("jobs-{name}"))).unwrap(),
            "2"
        );
    }
    let invalid_after_warm = run("0");
    assert!(!invalid_after_warm.status.success());
    assert!(String::from_utf8_lossy(&invalid_after_warm.stderr)
        .contains("MITTS_JOBS must be a positive integer"));
}
