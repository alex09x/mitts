/*
 * Mitts Build — tests for one Cargo configuration value
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::*;

#[test]
fn env_names_follow_cargo() {
    assert_eq!(env_var_name("build.jobs"), "CARGO_BUILD_JOBS");
    assert_eq!(
        env_var_name("build.rustc-wrapper"),
        "CARGO_BUILD_RUSTC_WRAPPER"
    );
    assert_eq!(
        env_var_name("target.x86_64-unknown-linux-gnu.runner"),
        "CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUNNER"
    );
    assert_eq!(env_var_name("alias.xtask"), "CARGO_ALIAS_XTASK");
}

#[test]
fn quoted_parts_keep_their_dots() {
    assert_eq!(
        split_key("target.'cfg(target_os = \"linux\")'.runner"),
        ["target", "cfg(target_os = \"linux\")", "runner"]
    );
    let table: toml::Value = toml::from_str("[target.'cfg(unix)']\nrunner = \"qemu\"\n").unwrap();
    assert_eq!(
        lookup(&table, "target.'cfg(unix)'.runner").and_then(|v| v.as_str()),
        Some("qemu")
    );
}

fn value(v: toml::Value) -> ConfigValue {
    ConfigValue {
        value: v,
        base: PathBuf::from("/w"),
        origin: "test".into(),
    }
}

#[test]
fn values_from_environment_strings_convert_like_cargo() {
    assert!(value(toml::Value::String("true".into())).as_bool().unwrap());
    assert!(value(toml::Value::String("yes".into())).as_bool().is_err());
    assert_eq!(
        value(toml::Value::String(" 8 ".into())).as_i64().unwrap(),
        8
    );
    assert_eq!(value(toml::Value::Integer(-2)).as_i64().unwrap(), -2);
    assert_eq!(
        value(toml::Value::String("qemu-riscv64 -L /sysroot".into()))
            .as_string_list()
            .unwrap(),
        ["qemu-riscv64", "-L", "/sysroot"]
    );
    let array = toml::Value::Array(vec!["a b".into(), "c".into()]);
    assert_eq!(value(array).as_string_list().unwrap(), ["a b", "c"]);
}

#[test]
fn programs_with_a_separator_are_relative_to_the_config() {
    let base = Path::new("/w");
    assert_eq!(
        ConfigValue::resolve_program(base, "qemu"),
        PathBuf::from("qemu")
    );
    assert_eq!(
        ConfigValue::resolve_program(base, "tools/run.sh"),
        PathBuf::from("/w/tools/run.sh")
    );
    assert_eq!(
        ConfigValue::resolve_program(base, "/usr/bin/x"),
        PathBuf::from("/usr/bin/x")
    );
}

#[test]
fn the_nearest_file_wins_and_tables_come_lowest_first() {
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("p");
    std::fs::create_dir_all(project.join(".cargo")).unwrap();
    std::fs::create_dir_all(root.path().join(".cargo")).unwrap();
    std::fs::write(
        root.path().join(".cargo/config.toml"),
        "[net]\nretry = 1\n[alias]\nx = \"build\"\n",
    )
    .unwrap();
    std::fs::write(
        project.join(".cargo/config.toml"),
        "[net]\nretry = 5\n[alias]\ny = \"test\"\n",
    )
    .unwrap();
    let found = get(&project, "net.retry").unwrap().unwrap();
    assert_eq!(found.as_i64().unwrap(), 5);
    assert_eq!(found.base, project);
    let aliases = tables(&project, "alias").unwrap();
    let names: Vec<_> = aliases
        .iter()
        .flat_map(|(t, _, _)| t.keys().cloned())
        .collect();
    assert!(names.ends_with(&["x".to_string(), "y".to_string()]));
}
