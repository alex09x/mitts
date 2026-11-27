/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Every stable option of every Cargo command, as Cargo's own command reference lists it
//! (`fixtures/cargo_stable_flags.json`, extracted from rust-lang/cargo
//! `src/doc/man/generated_txt`), is an option of the same Mitts command. `-C` is a global
//! option given before the command, as in Cargo; `--config` is accepted after any command.

use std::process::Command;

fn help(args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_mitts"))
        .args(args)
        .arg("--help")
        .output()
        .expect("run mitts");
    String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr)
}

fn mentions(text: &str, flag: &str) -> bool {
    text.match_indices(flag).any(|(i, _)| {
        let before = text[..i].chars().next_back();
        let after = text[i + flag.len()..].chars().next();
        let boundary = |c: Option<char>| c.is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '-'));
        boundary(before) && boundary(after)
    })
}

#[test]
fn every_stable_cargo_option_is_a_mitts_option() {
    let data: serde_json::Value =
        serde_json::from_str(include_str!("../fixtures/cargo_stable_flags.json")).unwrap();
    let commands = data["commands"].as_object().unwrap();
    assert!(commands.len() >= 35, "the whole command reference");
    let mut missing = Vec::new();
    let mut checked = 0;
    for (command, flags) in commands {
        let argv: Vec<&str> = command.split(' ').collect();
        let text = help(&argv);
        assert!(
            !text.contains("no such command"),
            "`mitts {command}` exists: {text}"
        );
        for flag in flags.as_array().unwrap() {
            let flag = flag.as_str().unwrap();
            checked += 1;
            match flag {
                // Global, before the command (`mitts -C DIR build`), as in Cargo.
                "-C" => {}
                "--config" => {
                    let mut with = argv.clone();
                    with.extend(["--config", "net.offline=true"]);
                    let text = help(&with);
                    if text.contains("unexpected argument") {
                        missing.push(format!("{command} --config"));
                    }
                }
                _ if !mentions(&text, flag) => missing.push(format!("{command} {flag}")),
                _ => {}
            }
        }
    }
    assert!(checked > 700, "{checked} options checked");
    assert!(missing.is_empty(), "Cargo options Mitts lacks: {missing:?}");
}

/// The options of a build are options of the commands that build, as in Cargo: `mitts fmt`,
/// `mitts gc` or `mitts login` do not list `--timings` (they did when the options were
/// global). `--slow-units` is Mitts' own and goes with `--timings`.
#[test]
fn build_options_are_options_of_the_commands_that_build_only() {
    let data: serde_json::Value =
        serde_json::from_str(include_str!("../fixtures/cargo_stable_flags.json")).unwrap();
    let mut commands: Vec<(String, Vec<String>)> = data["commands"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(c, flags)| {
            let flags = flags.as_array().unwrap();
            (
                c.clone(),
                flags
                    .iter()
                    .map(|f| f.as_str().unwrap().to_string())
                    .collect(),
            )
        })
        .collect();
    // Mitts' own commands that compile and are not in Cargo's reference.
    for own in ["clippy", "coverage", "llvm-cov"] {
        let check = commands
            .iter()
            .find(|(c, _)| c == "check")
            .unwrap()
            .1
            .clone();
        commands.push((own.to_string(), check));
    }
    // And the ones that compile nothing.
    for own in ["fmt", "explain", "gc"] {
        commands.push((own.to_string(), Vec::new()));
    }
    let mut extra = Vec::new();
    for (command, cargo_flags) in &commands {
        let argv: Vec<&str> = command.split(' ').collect();
        let text = help(&argv);
        let has = |flag: &str| cargo_flags.iter().any(|f| f == flag);
        let compiles = has("--timings");
        for (flag, allowed) in [
            ("--timings", compiles),
            ("--slow-units", compiles),
            // Mitts reports it after any build; Cargo lists it for build, check, rustc, test.
            ("--future-incompat-report", compiles),
            ("--ignore-rust-version", has("--ignore-rust-version")),
        ] {
            if !allowed && mentions(&text, flag) {
                extra.push(format!("{command} {flag}"));
            }
        }
    }
    assert!(
        extra.is_empty(),
        "build options of commands that do not build: {extra:?}"
    );
}

#[test]
fn the_directory_option_goes_before_the_command() {
    let temp = tempfile::TempDir::new().unwrap();
    std::fs::write(
        temp.path().join("Cargo.toml"),
        "[package]\nname = \"elsewhere\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(temp.path().join("src")).unwrap();
    std::fs::write(temp.path().join("src/lib.rs"), "").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_mitts"))
        .arg("-C")
        .arg(temp.path())
        .arg("locate-project")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("Cargo.toml"));
}
