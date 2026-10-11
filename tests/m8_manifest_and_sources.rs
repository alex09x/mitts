/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::rust::manifest::CargoToml;
use mitts::adapter::rust::source_lookup::find_crate_source_dir;
use std::fs;

#[test]
fn test_m8_manifest_parsing_and_aliases() {
    let toml_str = r#"
[package]
name = "demo-pkg"
version = "0.2.0"
edition = "2021"
build = "custom_build.rs"

[dependencies]
serde_orig = { package = "serde", version = "1.0" }
itoa = "1.0"

[build-dependencies]
cc_orig = { package = "cc", version = "1.0" }

[dev-dependencies]
temp_orig = { package = "tempfile", version = "3.0" }

[target.'cfg(target_os = "linux")'.dependencies]
libc_orig = { package = "libc", version = "0.2" }
"#;

    let manifest = CargoToml::parse(toml_str).expect("Valid manifest parse");
    assert!(manifest.package.is_some());
    let pkg = manifest.package.as_ref().unwrap();
    assert_eq!(pkg.name, "demo-pkg");
    assert_eq!(pkg.version.as_deref(), Some("0.2.0"));

    let aliases = manifest.dependency_aliases();
    assert_eq!(
        aliases.alias_to_real.get("serde_orig").map(|s| s.as_str()),
        Some("serde")
    );
    assert_eq!(
        aliases.alias_to_real.get("cc_orig").map(|s| s.as_str()),
        Some("cc")
    );
    assert_eq!(
        aliases.alias_to_real.get("temp_orig").map(|s| s.as_str()),
        Some("tempfile")
    );
    assert_eq!(
        aliases.alias_to_real.get("libc_orig").map(|s| s.as_str()),
        Some("libc")
    );

    let temp = tempfile::tempdir().expect("tempdir");
    fs::write(temp.path().join("custom_build.rs"), "fn main() {}").unwrap();
    assert!(manifest.has_build_script(temp.path()));
}

#[test]
fn test_m8_manifest_build_script_variants() {
    let disabled_toml = r#"
[package]
name = "no-build"
version = "0.1.0"
edition = "2021"
build = false
"#;
    let manifest = CargoToml::parse(disabled_toml).expect("Parse disabled build");
    let temp = tempfile::tempdir().expect("tempdir");
    fs::write(temp.path().join("build.rs"), "fn main() {}").unwrap();
    assert!(!manifest.has_build_script(temp.path()));

    let default_toml = r#"
[package]
name = "default-build"
version = "0.1.0"
edition = "2021"
"#;
    let def_manifest = CargoToml::parse(default_toml).expect("Parse default build");
    assert!(def_manifest.has_build_script(temp.path()));
}

#[test]
fn test_m8_source_lookup_registry_and_git() {
    let temp_home = tempfile::tempdir().expect("temp home");
    let reg_dir = temp_home
        .path()
        .join(".cargo")
        .join("registry")
        .join("src")
        .join("index.crates.io-6f17d22bba15001f")
        .join("foo-bar-1.2.3");
    fs::create_dir_all(&reg_dir).unwrap();
    fs::write(
        reg_dir.join("Cargo.toml"),
        "[package]\nname = \"foo-bar\"\nversion = \"1.2.3\"\n",
    )
    .unwrap();

    let git_dir = temp_home
        .path()
        .join(".cargo")
        .join("git")
        .join("checkouts")
        .join("my-repo-12345")
        .join("abcdef12");
    fs::create_dir_all(&git_dir).unwrap();
    fs::write(
        git_dir.join("Cargo.toml"),
        "[package]\nname = \"my-git-crate\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();

    // Set HOME to temp_home
    let orig_home = std::env::var("HOME").ok();
    std::env::set_var("HOME", temp_home.path());

    let found_reg = find_crate_source_dir("foo-bar", "1.2.3", None);
    assert!(found_reg.is_some(), "Should find registry crate");
    assert_eq!(found_reg.unwrap(), reg_dir);

    let found_git = find_crate_source_dir(
        "my-git-crate",
        "0.1.0",
        Some("git+https://github.com/test/repo#abcdef123456"),
    );
    assert!(found_git.is_some(), "Should find git checkout");
    assert_eq!(found_git.unwrap(), git_dir);

    // Restore HOME
    if let Some(h) = orig_home {
        std::env::set_var("HOME", h);
    }
}

#[test]
fn test_m8_manifest_discover_targets_and_workspaces() {
    let temp = tempfile::tempdir().expect("tempdir");
    let pkg_dir = temp.path();

    let toml_str = r#"
[package]
name = "workspace-root"
version = "0.1.0"
edition = "2021"

[workspace]
members = ["crates/*", "subpkg"]

[lib]
name = "custom_lib"
proc-macro = true

[[bin]]
name = "my-cli"
path = "src/bin/cli.rs"

[[test]]
name = "integ_test"
path = "tests/integ.rs"
"#;

    // Create directories and mock files
    fs::create_dir_all(pkg_dir.join("src").join("bin")).unwrap();
    fs::create_dir_all(pkg_dir.join("tests")).unwrap();
    fs::create_dir_all(pkg_dir.join("crates").join("child1")).unwrap();
    fs::create_dir_all(pkg_dir.join("subpkg")).unwrap();

    fs::write(pkg_dir.join("src").join("lib.rs"), "pub fn lib() {}").unwrap();
    fs::write(
        pkg_dir.join("src").join("bin").join("cli.rs"),
        "fn main() {}",
    )
    .unwrap();
    fs::write(pkg_dir.join("tests").join("integ.rs"), "fn main() {}").unwrap();

    fs::write(
        pkg_dir.join("crates").join("child1").join("Cargo.toml"),
        "[package]\nname = \"child1\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(
        pkg_dir.join("subpkg").join("Cargo.toml"),
        "[package]\nname = \"subpkg\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();

    let manifest = CargoToml::parse(toml_str).expect("Valid manifest parse");

    // 1. Discover targets
    let targets = manifest.discover_targets(pkg_dir);
    assert!(targets
        .iter()
        .any(|t| t.name == "custom_lib" && t.kind == "proc-macro"));
    assert!(targets
        .iter()
        .any(|t| t.name == "my-cli" && t.kind == "bin"));

    // 2. Discover test targets
    let test_targets = manifest.discover_test_targets(pkg_dir);
    assert!(test_targets
        .iter()
        .any(|t| t.name == "workspace_root_unit_test"));
    assert!(test_targets.iter().any(|t| t.name == "integ_test"));

    // 3. Discover workspace members
    let members = manifest.discover_workspace_members(pkg_dir).unwrap();
    let member_names: Vec<_> = members.iter().map(|(n, _)| n.as_str()).collect();
    assert!(member_names.contains(&"child1"));
    assert!(member_names.contains(&"subpkg"));
    assert!(member_names.contains(&"workspace-root"));
}
