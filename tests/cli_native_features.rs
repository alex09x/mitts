/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;

fn mitts_binary() -> PathBuf {
    PathBuf::from(
        std::env::var_os("CARGO_BIN_EXE_mitts")
            .expect("Mitts binary is provided to integration tests"),
    )
}

fn invoke_mitts(directory: &Path, args: &[&str]) -> Output {
    Command::new(mitts_binary())
        .args(args)
        .current_dir(directory)
        .output()
        .expect("run Mitts CLI")
}

fn host_triple() -> String {
    let output = Command::new("rustc")
        .arg("-vV")
        .output()
        .expect("run rustc -vV");
    assert!(output.status.success());
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .find_map(|line| line.strip_prefix("host: ").map(str::to_string))
        .expect("rustc host triple")
}

fn write_file(path: impl AsRef<Path>, contents: &str) {
    let path = path.as_ref();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

#[test]
fn examples_target_triple_quiet_and_program_args_work_together() {
    let temp = TempDir::new().unwrap();
    write_file(
        temp.path().join("Cargo.toml"),
        "[package]\nname = \"example-fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[features]\npty = []\n[[example]]\nname = \"headless\"\nrequired-features = [\"pty\"]\n",
    );
    write_file(temp.path().join("src/lib.rs"), "pub fn library() {}\n");
    write_file(
        temp.path().join("examples/headless.rs"),
        "fn main() { println!(\"program:{}\", std::env::args().nth(1).unwrap_or_default()); }\n",
    );
    let manifest = temp.path().join("Cargo.toml").display().to_string();
    let target_dir = temp.path().join("target").display().to_string();
    let cache_dir = temp.path().join("mitts-cache").display().to_string();
    let target = host_triple();
    let linker_script = temp.path().join("linker.sh");
    let linker_log = temp.path().join("linker.log");
    write_file(
        &linker_script,
        &format!(
            "#!/bin/sh\necho invoked >> '{}'\nexec cc \"$@\"\n",
            linker_log.display()
        ),
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&linker_script).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&linker_script, permissions).unwrap();
    }
    write_file(
        temp.path().join(".cargo/config.toml"),
        &format!(
            "[target.\"{target}\"]\nlinker = \"{}\"\n",
            linker_script.display()
        ),
    );

    let missing = invoke_mitts(
        temp.path(),
        &[
            "build",
            "--quiet",
            "--manifest-path",
            &manifest,
            "--target-dir",
            &target_dir,
            "--cache-dir",
            &cache_dir,
            "--example",
            "missing",
        ],
    );
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("example target named `missing`"));

    let missing_feature = invoke_mitts(
        temp.path(),
        &[
            "build",
            "--quiet",
            "--manifest-path",
            &manifest,
            "--target-dir",
            &target_dir,
            "--cache-dir",
            &cache_dir,
            "--example",
            "headless",
        ],
    );
    assert!(!missing_feature.status.success());
    let missing_feature_output = format!(
        "{}{}",
        String::from_utf8_lossy(&missing_feature.stdout),
        String::from_utf8_lossy(&missing_feature.stderr)
    );
    assert!(
        missing_feature_output.contains("selected package"),
        "unexpected diagnostic: {missing_feature_output}"
    );

    let build = invoke_mitts(
        temp.path(),
        &[
            "build",
            "-q",
            "--manifest-path",
            &manifest,
            "--target-dir",
            &target_dir,
            "--cache-dir",
            &cache_dir,
            "--target",
            &target,
            "--example",
            "headless",
            "--features",
            "pty",
        ],
    );
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    assert!(
        build.stdout.is_empty(),
        "quiet success wrote stdout: {:?}",
        build.stdout
    );
    assert!(
        build.stderr.is_empty(),
        "quiet success wrote stderr: {:?}",
        build.stderr
    );
    assert!(temp
        .path()
        .join("target")
        .join(&target)
        .join("debug/examples/headless")
        .is_file());
    assert!(
        linker_log.is_file(),
        "configured target linker was not invoked"
    );

    let run = invoke_mitts(
        temp.path(),
        &[
            "run",
            "--quiet",
            "--manifest-path",
            &manifest,
            "--target-dir",
            &target_dir,
            "--cache-dir",
            &cache_dir,
            "--target",
            &target,
            "--example",
            "headless",
            "--features",
            "pty",
            "--",
            "argument",
        ],
    );
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&run.stdout), "program:argument\n");
}

#[test]
fn metadata_json_filters_optional_and_foreign_target_dependencies() {
    let temp = TempDir::new().unwrap();
    write_file(
        temp.path().join("Cargo.toml"),
        "[package]\nname = \"metadata-fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[features]\nssh = [\"dep:optional-dep\"]\n[dependencies]\noptional-dep = { path = \"optional-dep\", optional = true }\n[target.'cfg(target_os = \"macos\")'.dependencies]\nmac-dep = { path = \"mac-dep\" }\n[target.'cfg(target_os = \"windows\")'.dependencies]\nwin-dep = { path = \"win-dep\" }\n",
    );
    write_file(temp.path().join("src/lib.rs"), "pub fn root() {}\n");
    for name in ["optional-dep", "mac-dep", "win-dep"] {
        write_file(
            temp.path().join(name).join("Cargo.toml"),
            &format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
        );
        write_file(
            temp.path().join(name).join("src/lib.rs"),
            "pub fn dep() {}\n",
        );
    }
    let manifest = temp.path().join("Cargo.toml").display().to_string();
    let cache_dir = temp.path().join("mitts-cache").display().to_string();
    let output = invoke_mitts(
        temp.path(),
        &[
            "metadata",
            "--format-version",
            "1",
            "--manifest-path",
            &manifest,
            "--cache-dir",
            &cache_dir,
            "--features",
            "ssh",
            "--filter-platform",
            "aarch64-apple-darwin",
            "--offline",
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["version"], 1);
    let package_names: Vec<_> = json["packages"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|package| package["name"].as_str())
        .collect();
    assert!(package_names.contains(&"metadata-fixture"));
    assert!(package_names.contains(&"optional-dep"));
    assert!(package_names.contains(&"mac-dep"));
    assert!(!package_names.contains(&"win-dep"));
    let nodes = json["resolve"]["nodes"].as_array().unwrap();
    let root = json["packages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|package| package["name"] == "metadata-fixture")
        .unwrap();
    let root_node = nodes.iter().find(|node| node["id"] == root["id"]).unwrap();
    assert!(root_node["features"]
        .as_array()
        .is_some_and(|features| features.iter().any(|feature| feature == "ssh")));
    let dependency_names = root_node["deps"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|dependency| dependency["name"].as_str())
        .collect::<Vec<_>>();
    assert!(dependency_names.contains(&"optional-dep"));
    assert!(dependency_names.contains(&"mac-dep"));
    assert!(!dependency_names.contains(&"win-dep"));
    let mac_dependency = root_node["deps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|dependency| dependency["name"] == "mac-dep")
        .unwrap();
    assert!(mac_dependency["dep_kinds"][0]["target"]
        .as_str()
        .is_some_and(|target| target.contains("target_os") && target.contains("macos")));
}

#[test]
fn rustc_native_static_libs_output_links_a_c_consumer() {
    let temp = TempDir::new().unwrap();
    write_file(
        temp.path().join("Cargo.toml"),
        "[package]\nname = \"staticlib-fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write_file(
        temp.path().join("src/lib.rs"),
        "#[no_mangle]\npub extern \"C\" fn answer() -> i32 { 42 }\n",
    );
    let manifest = temp.path().join("Cargo.toml").display().to_string();
    let target_dir = temp.path().join("target").display().to_string();
    let cache_dir = temp.path().join("mitts-cache").display().to_string();
    let output = invoke_mitts(
        temp.path(),
        &[
            "rustc",
            "--release",
            "--lib",
            "--manifest-path",
            &manifest,
            "--target-dir",
            &target_dir,
            "--cache-dir",
            &cache_dir,
            "--crate-type",
            "staticlib",
            "--",
            "--print",
            "native-static-libs",
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let diagnostics = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let link_line = diagnostics
        .lines()
        .find_map(|line| {
            line.split_once("native-static-libs:")
                .map(|(_, flags)| flags.trim())
        })
        .expect("rustc reports native-static-libs");
    let archive = temp.path().join("target/release/libstaticlib_fixture.a");
    assert!(archive.is_file(), "missing {}", archive.display());
    let c_source = temp.path().join("consumer.c");
    write_file(
        &c_source,
        "extern int answer(void); int main(void) { return answer() != 42; }\n",
    );
    let executable = temp.path().join("consumer");
    let linked = Command::new("cc")
        .arg(&c_source)
        .arg(&archive)
        .args(link_line.split_whitespace())
        .arg("-o")
        .arg(&executable)
        .output()
        .expect("invoke the system C linker");
    assert!(
        linked.status.success(),
        "{}",
        String::from_utf8_lossy(&linked.stderr)
    );
    let run = Command::new(executable).status().expect("run C consumer");
    assert!(run.success());
}
