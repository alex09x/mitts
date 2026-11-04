/*
 * Mitts Build — package command integration
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::process::Command;

#[test]
fn package_and_publish_help_explain_supported_modes() {
    for command in ["package", "publish"] {
        let output = Command::new(env!("CARGO_BIN_EXE_mitts"))
            .args([command, "--help"])
            .output()
            .unwrap();
        assert!(output.status.success());
        let help = String::from_utf8_lossy(&output.stdout);
        for flag in ["--locked", "--offline", "--frozen", "--cache-dir"] {
            assert!(help.contains(flag), "missing {flag}: {help}");
        }
        if command == "package" {
            assert!(help.contains("Include/exclude"));
        }
        if command == "publish" {
            assert!(help.contains("CARGO_REGISTRY_TOKEN"));
        }
    }
}

#[test]
fn locked_package_rejects_missing_optional_dependency() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("source");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("Cargo.toml"), "[package]\nname='root'\nversion='0.1.0'\nedition='2021'\n[dependencies]\noptional_dep={version='1',optional=true}\n").unwrap();
    std::fs::write(
        root.join("Cargo.lock"),
        "version=4\n[[package]]\nname=\"root\"\nversion=\"0.1.0\"\n",
    )
    .unwrap();
    std::fs::write(root.join("src/lib.rs"), "pub fn f() {}\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_mitts"))
        .current_dir(&root)
        .args(["package", "--locked", "--no-verify", "--cache-dir"])
        .arg(temp.path().join("cache"))
        .arg("--target-dir")
        .arg(temp.path().join("target"))
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "locked packaging accepted an incomplete lockfile"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("Cargo.lock does not match"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn package_locked_rejects_mismatched_root_and_default_repairs_archive() {
    use std::io::Read;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("source");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname='root'\nversion='0.1.0'\nedition='2021'\n",
    )
    .unwrap();
    let stale = "version=4\n[[package]]\nname='root'\nversion='0.0.1'\n";
    std::fs::write(root.join("Cargo.lock"), stale).unwrap();
    std::fs::write(root.join("src/lib.rs"), "pub fn f() {}\n").unwrap();
    let target = temp.path().join("target");
    let run = |locked: bool| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_mitts"));
        command
            .current_dir(&root)
            .args(["package", "--no-verify", "--cache-dir"])
            .arg(temp.path().join("cache"))
            .arg("--target-dir")
            .arg(&target);
        if locked {
            command.arg("--locked");
        }
        command.output().unwrap()
    };
    let rejected = run(true);
    assert!(!rejected.status.success());
    let result = run(false);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(root.join("Cargo.lock")).unwrap(),
        stale
    );
    let file = std::fs::File::open(target.join("package/root-0.1.0.crate")).unwrap();
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(file));
    let mut found = false;
    for entry in archive.entries().unwrap() {
        let mut entry = entry.unwrap();
        if entry.path().unwrap().ends_with("Cargo.lock") {
            let mut text = String::new();
            entry.read_to_string(&mut text).unwrap();
            assert!(text.contains("0.1.0"));
            found = true;
        }
    }
    assert!(found);
}

#[test]
fn packages_directory_without_git_and_skips_hidden_files() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("source");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname='plain'\nversion='0.1.0'\nedition='2021'\n",
    )
    .unwrap();
    std::fs::write(root.join("src/lib.rs"), "pub fn f() {}\n").unwrap();
    std::fs::write(root.join(".private"), "hidden\n").unwrap();
    let output_dir = temp.path().join("output");
    let output = Command::new(env!("CARGO_BIN_EXE_mitts"))
        .current_dir(&root)
        .arg("package")
        .arg("--target-dir")
        .arg(&output_dir)
        .arg("--cache-dir")
        .arg(temp.path().join("cache"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let file = std::fs::File::open(output_dir.join("package/plain-0.1.0.crate")).unwrap();
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(file));
    let paths: Vec<_> = archive
        .entries()
        .unwrap()
        .map(|entry| entry.unwrap().path().unwrap().to_path_buf())
        .collect();
    assert!(paths.contains(&"plain-0.1.0/src/lib.rs".into()));
    assert!(paths.contains(&"plain-0.1.0/Cargo.lock".into()));
    assert!(!paths.contains(&"plain-0.1.0/.private".into()));
}

#[test]
fn offline_package_refuses_an_uncached_dependency() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("source");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("Cargo.toml"), "[package]\nname='offline-fixture'\nversion='0.1.0'\nedition='2021'\n[dependencies]\nmitts-nonexistent-package-for-offline-test='=0.0.1'\n").unwrap();
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
            .current_dir(&root)
            .args(args)
            .status()
            .unwrap()
            .success());
    }
    let target = temp.path().join("target");
    let output = Command::new(env!("CARGO_BIN_EXE_mitts"))
        .current_dir(&root)
        .args(["package", "--offline", "--no-verify", "--cache-dir"])
        .arg(temp.path().join("cache"))
        .arg("--target-dir")
        .arg(&target)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!target.join("package/offline-fixture-0.1.0.crate").exists());
}

#[test]
fn package_generates_missing_lock_without_modifying_checkout() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("source");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname='unlocked'\nversion='0.1.0'\nedition='2021'\n",
    )
    .unwrap();
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
            .current_dir(&root)
            .args(args)
            .status()
            .unwrap()
            .success());
    }
    let output_dir = temp.path().join("output");
    for flag in ["--locked", "--frozen"] {
        let rejected = Command::new(env!("CARGO_BIN_EXE_mitts"))
            .current_dir(&root)
            .args(["package", flag, "--no-verify"])
            .arg("--target-dir")
            .arg(&output_dir)
            .arg("--cache-dir")
            .arg(temp.path().join("cache"))
            .output()
            .unwrap();
        assert!(!rejected.status.success());
        assert!(String::from_utf8_lossy(&rejected.stderr).contains("Cargo.lock is missing"));
    }
    let output = Command::new(env!("CARGO_BIN_EXE_mitts"))
        .current_dir(&root)
        .arg("package")
        .arg("--target-dir")
        .arg(&output_dir)
        .arg("--cache-dir")
        .arg(temp.path().join("cache"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!root.join("Cargo.lock").exists());
    let file = std::fs::File::open(output_dir.join("package/unlocked-0.1.0.crate")).unwrap();
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(file));
    assert!(archive.entries().unwrap().any(|entry| entry
        .unwrap()
        .path()
        .unwrap()
        .ends_with("Cargo.lock")));
}

#[test]
fn package_verifies_extracted_crate_and_publish_dry_run() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("source");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname='example-package'\nversion='0.1.0'\nedition='2021'\n",
    )
    .unwrap();
    std::fs::write(
        root.join("Cargo.lock"),
        "version = 4\n\n[[package]]\nname = 'example-package'\nversion = '0.1.0'\n",
    )
    .unwrap();
    std::fs::write(root.join("src/lib.rs"), "pub fn answer() -> u8 { 42 }\n").unwrap();
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
            .current_dir(&root)
            .args(args)
            .status()
            .unwrap()
            .success());
    }
    let target = temp.path().join("target");
    let output = Command::new(env!("CARGO_BIN_EXE_mitts"))
        .current_dir(&root)
        .arg("package")
        .arg("--target-dir")
        .arg(&target)
        .arg("--cache-dir")
        .arg(temp.path().join("verification-cache"))
        .env_remove("CARGO_REGISTRY_TOKEN")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let archive = target.join("package/example-package-0.1.0.crate");
    assert!(archive.is_file());
    let output = Command::new(env!("CARGO_BIN_EXE_mitts"))
        .current_dir(&root)
        .args(["publish", "--dry-run", "--archive"])
        .arg(&archive)
        .arg("--cache-dir")
        .arg(temp.path().join("verification-cache"))
        .env_remove("CARGO_REGISTRY_TOKEN")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn package_rejects_build_script_that_changes_source() {
    let temp = tempfile::tempdir().unwrap();
    let scratch = temp.path().join("scratch");
    std::fs::create_dir(&scratch).unwrap();
    let root = temp.path().join("project");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname='mutating-package'\nversion='0.1.0'\nedition='2021'\n",
    )
    .unwrap();
    std::fs::write(
        root.join("Cargo.lock"),
        "version=4\n[[package]]\nname='mutating-package'\nversion='0.1.0'\n",
    )
    .unwrap();
    std::fs::write(root.join("src/lib.rs"), "pub fn f() {}\n").unwrap();
    std::fs::write(
        root.join("build.rs"),
        "fn main() { std::fs::write(\"src/lib.rs\", \"pub fn modified() {}\\n\").unwrap(); }\n",
    )
    .unwrap();
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
            .current_dir(&root)
            .args(args)
            .status()
            .unwrap()
            .success());
    }
    let output = Command::new(env!("CARGO_BIN_EXE_mitts"))
        .current_dir(&root)
        .arg("package")
        .arg("--target-dir")
        .arg(temp.path().join("target"))
        .env("TMPDIR", &scratch)
        .env_remove("CARGO_REGISTRY_TOKEN")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("modified its source files"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(std::fs::read_dir(&scratch).unwrap().count(), 0);
}

#[test]
fn publish_offline_refuses_network_before_packaging() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(
        temp.path().join("Cargo.toml"),
        "[package]\nname='demo'\nversion='0.1.0'\n",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_mitts"))
        .current_dir(temp.path())
        .args(["publish", "--offline", "--no-verify"])
        .env_remove("CARGO_REGISTRY_TOKEN")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("publication requires network access"));
    assert!(!temp.path().join("target").exists());
}

#[test]
fn publish_rejects_a_manifest_that_disables_publication() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(
        temp.path().join("Cargo.toml"),
        "[package]\nname='private'\nversion='0.1.0'\npublish=false\n",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_mitts"))
        .current_dir(temp.path())
        .args(["publish", "--dry-run"])
        .env_remove("CARGO_REGISTRY_TOKEN")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("does not allow publication"));
}
