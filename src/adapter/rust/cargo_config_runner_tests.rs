/*
 * Mitts Build — tests for `target.<triple>.runner`
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::*;

fn project(config: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".cargo")).unwrap();
    std::fs::write(dir.path().join(".cargo/config.toml"), config).unwrap();
    dir
}

#[test]
fn a_triple_runner_wins_and_relative_programs_start_beside_the_cargo_dir() {
    let dir = project(
        "[target.zz-test-target]\nrunner = [\"tools/emu\", \"--fast\"]\n\
         [target.'cfg(all())']\nrunner = \"never-used\"\n",
    );
    let runner = runner(dir.path(), "zz-test-target").unwrap().unwrap();
    assert_eq!(runner[0], dir.path().join("tools/emu").to_string_lossy());
    assert_eq!(runner[1], "--fast");
}

#[test]
fn one_matching_cfg_runner_applies_and_two_are_an_error() {
    let host = crate::adapter::rust::compiler_resolve::try_resolve_host_triple().unwrap();
    let dir = project("[target.'cfg(all())']\nrunner = \"emu -x\"\n");
    assert_eq!(runner(dir.path(), &host).unwrap().unwrap(), ["emu", "-x"]);

    let dir = project("[target.'cfg(any())']\nrunner = \"no\"\n");
    assert_eq!(runner(dir.path(), &host).unwrap(), None);

    let dir = project(
        "[target.'cfg(all())']\nrunner = \"a\"\n[target.'cfg(not(any()))']\nrunner = \"b\"\n",
    );
    let error = runner(dir.path(), &host).unwrap_err().to_string();
    assert!(error.contains("several matching"), "{error}");
}
