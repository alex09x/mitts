/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! `--profile NAME` (issue #109): custom profiles with `inherits` chains, built into
//! `target/NAME`, and the built-in names.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use tempfile::TempDir;

fn write_file(path: impl AsRef<Path>, contents: &str) {
    let path = path.as_ref();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// A binary that reports its debug assertions and, through its build script, the
/// OPT_LEVEL and PROFILE build scripts were given.
fn project() -> TempDir {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    write_file(
        root.join("Cargo.toml"),
        "[package]\nname = \"p\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\
         [profile.ci]\ninherits = \"release\"\ndebug-assertions = true\n\
         [profile.ci-fast]\ninherits = \"ci\"\nopt-level = 1\n\
         [profile.broken]\nopt-level = 2\n",
    );
    write_file(
        root.join("build.rs"),
        "fn main() {\n\
         println!(\"cargo:rustc-env=SEEN_OPT={}\", std::env::var(\"OPT_LEVEL\").unwrap());\n\
         println!(\"cargo:rustc-env=SEEN_PROFILE={}\", std::env::var(\"PROFILE\").unwrap());\n}\n",
    );
    write_file(
        root.join("src/main.rs"),
        "fn main() {\n\
         println!(\"assertions={} opt={} profile={}\", cfg!(debug_assertions), \
         env!(\"SEEN_OPT\"), env!(\"SEEN_PROFILE\"));\n}\n\
         #[test]\nfn assertions_are_on() {\n    assert!(cfg!(debug_assertions));\n}\n",
    );
    temp
}

/// A profile with the same settings as `release` shares its cached build-script run; the
/// restored OUT_DIR must still hold the generated file (review of #114, P1).
#[test]
fn a_shared_build_script_entry_keeps_its_out_dir_across_profiles() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    write_file(
        root.join("Cargo.toml"),
        "[package]\nname = \"g\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\
         [profile.same]\ninherits = \"release\"\n",
    );
    write_file(
        root.join("build.rs"),
        "fn main() {\n\
         let out = std::env::var(\"OUT_DIR\").unwrap();\n\
         std::fs::write(std::path::Path::new(&out).join(\"gen.rs\"), \"pub const V: u32 = 7;\").unwrap();\n}\n",
    );
    write_file(
        root.join("src/main.rs"),
        "include!(concat!(env!(\"OUT_DIR\"), \"/gen.rs\"));\nfn main() { println!(\"v={}\", V); }\n",
    );
    assert_ok(&mitts(root, &["build", "-q", "--release"]));
    assert_ok(&mitts(root, &["build", "-q", "--profile", "same"]));
    assert_eq!(run_binary(&root.join("target/same/g")), "v=7\n");
}

/// Profiles a selected one inherits from are validated too, and `inherits` may not name a
/// reserved profile (review of #114, P2).
#[test]
fn inherited_profiles_are_validated() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    write_file(
        root.join("Cargo.toml"),
        "[package]\nname = \"v\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\
         [profile.base]\ninherits = \"release\"\nopt-level = 9\n\
         [profile.child]\ninherits = \"base\"\n\
         [profile.debug]\ninherits = \"dev\"\n\
         [profile.sneaky]\ninherits = \"debug\"\n",
    );
    write_file(root.join("src/main.rs"), "fn main() {}\n");
    let child = mitts(root, &["build", "--profile", "child"]);
    assert!(!child.status.success());
    assert!(String::from_utf8_lossy(&child.stderr).contains("profile.base.opt-level"));
    let sneaky = mitts(root, &["build", "--profile", "sneaky"]);
    assert!(!sneaky.status.success());
    assert!(String::from_utf8_lossy(&sneaky.stderr).contains("reserved"));
}

/// A library caller passing `profile` gets the same build as `--profile` (review of #114, P3).
#[test]
fn library_callers_get_the_profile_normalized() {
    let temp = project();
    let root = temp.path();
    let opt = mitts::adapter::rust::types::RustBuildOptions {
        manifest_path: Some(root.join("Cargo.toml")),
        cache_dir: Some(root.join("mitts-cache")),
        profile: Some("ci".to_string()),
        native: true,
        ..Default::default()
    };
    let (_, result) = mitts::adapter::rust::RustAdapter::build(&opt).unwrap();
    assert!(result.success, "{}", result.stderr);
    assert_eq!(
        run_binary(&root.join("target/ci/p")),
        "assertions=true opt=3 profile=release\n"
    );
    mitts::adapter::rust::profile::set_custom_profile(None);
}

/// `mitts clean` removes what a custom profile built (review of #114, P4).
#[test]
fn clean_removes_custom_profile_outputs() {
    let temp = project();
    let root = temp.path();
    assert_ok(&mitts(root, &["build", "-q", "--profile", "ci"]));
    assert!(root.join("target/ci/p").is_file());
    assert_ok(&mitts(root, &["clean"]));
    assert!(!root.join("target/ci/p").exists());
}

#[test]
fn test_uses_the_profile() {
    let temp = project();
    let root = temp.path();
    // `ci` inherits release but turns debug assertions on, so the test passes.
    assert_ok(&mitts(root, &["test", "-q", "--profile", "ci"]));
    assert!(root.join("target/ci").is_dir());
    // Plain release has them off.
    assert!(!mitts(root, &["test", "-q", "--release"]).status.success());
}

fn mitts(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mitts"))
        .args(args)
        .arg("--cache-dir")
        .arg(root.join("mitts-cache"))
        .current_dir(root)
        .env_remove("CARGO_TARGET_DIR")
        .env_remove("CARGO_BUILD_TARGET_DIR")
        .output()
        .expect("run Mitts")
}

fn run_binary(path: &Path) -> String {
    let output = Command::new(path).output().expect("run the binary");
    assert!(output.status.success());
    String::from_utf8(output.stdout).unwrap()
}

fn assert_ok(output: &Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn a_custom_profile_builds_into_its_own_directory_with_its_settings() {
    let temp = project();
    let root = temp.path();
    assert_ok(&mitts(root, &["build", "-q", "--profile", "ci"]));
    // inherits release (PROFILE=release, opt 3), overrides debug-assertions.
    assert_eq!(
        run_binary(&root.join("target/ci/p")),
        "assertions=true opt=3 profile=release\n"
    );

    assert_ok(&mitts(root, &["build", "-q", "--profile", "ci-fast"]));
    // two levels: ci-fast -> ci -> release.
    assert_eq!(
        run_binary(&root.join("target/ci-fast/p")),
        "assertions=true opt=1 profile=release\n"
    );

    // The release build next to them is unaffected.
    assert_ok(&mitts(root, &["build", "-q", "--release"]));
    assert_eq!(
        run_binary(&root.join("target/release/p")),
        "assertions=false opt=3 profile=release\n"
    );
}

/// A build script sees OPT_LEVEL from the package's profile, and a cached run is not reused
/// after the profile's `opt-level` changes.
#[test]
fn build_scripts_see_the_profile_opt_level_and_rerun_when_it_changes() {
    let temp = project();
    let root = temp.path();
    assert_ok(&mitts(root, &["build", "-q", "--release"]));
    assert_eq!(
        run_binary(&root.join("target/release/p")),
        "assertions=false opt=3 profile=release\n"
    );
    let manifest = fs::read_to_string(root.join("Cargo.toml")).unwrap();
    fs::write(
        root.join("Cargo.toml"),
        format!("{manifest}[profile.release]\nopt-level = 2\n"),
    )
    .unwrap();
    assert_ok(&mitts(root, &["build", "-q", "--release"]));
    assert_eq!(
        run_binary(&root.join("target/release/p")),
        "assertions=false opt=2 profile=release\n"
    );
}

#[test]
fn built_in_profile_names_map_to_the_usual_builds() {
    let temp = project();
    let root = temp.path();
    assert_ok(&mitts(root, &["build", "-q", "--profile", "release"]));
    assert!(root.join("target/release/p").is_file());
    assert_ok(&mitts(root, &["build", "-q", "--profile", "dev"]));
    assert_eq!(
        run_binary(&root.join("target/debug/p")),
        "assertions=true opt=0 profile=debug\n"
    );
}

#[test]
fn run_uses_the_profile() {
    let temp = project();
    let root = temp.path();
    let output = mitts(root, &["run", "-q", "--profile", "ci-fast"]);
    assert_ok(&output);
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "assertions=true opt=1 profile=release\n"
    );
}

#[test]
fn undefined_uninherited_and_conflicting_profiles_are_errors() {
    let temp = project();
    let root = temp.path();
    for (args, expected) in [
        (&["build", "--profile", "nope"][..], "not defined"),
        (&["build", "--profile", "broken"][..], "inherits"),
        (
            &["build", "--release", "--profile", "ci"][..],
            "conflicting",
        ),
        (&["build", "--profile", "debug"][..], "reserved"),
    ] {
        let output = mitts(root, args);
        assert!(!output.status.success(), "{args:?} succeeded");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(expected), "{args:?}: {stderr}");
    }
}
