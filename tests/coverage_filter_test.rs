/*
 * Mitts Build — coverage filtering regression
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::process::Command;

#[test]
fn coverage_excludes_cached_sources_and_user_pattern_from_gate_and_lcov() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    let cache = temp.path().join("cache.with+characters");
    let dep = cache.join("sources/dependency");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join("tests")).unwrap();
    std::fs::create_dir_all(dep.join("src")).unwrap();
    std::fs::write(
        dep.join("Cargo.toml"),
        "[package]\nname='dependency'\nversion='0.1.0'\nedition='2021'\n",
    )
    .unwrap();
    std::fs::write(dep.join("src/lib.rs"), "pub fn value() -> u32 { 1 }\n").unwrap();
    std::fs::write(root.join("Cargo.toml"), format!("[package]\nname='coverage-filter'\nversion='0.1.0'\nedition='2021'\n[dependencies]\ndependency={{path={:?}}}\n", dep)).unwrap();
    std::fs::write(
        root.join("src/lib.rs"),
        "pub fn value() -> u32 { dependency::value() }\n",
    )
    .unwrap();
    std::fs::write(
        root.join("tests/sample.rs"),
        "#[test] fn sample() { assert_eq!(coverage_filter::value(), 1); }\n",
    )
    .unwrap();
    let run = |extra: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_mitts"))
            .current_dir(&root)
            .args(["llvm-cov", "--offline", "--test", "sample", "--cache-dir"])
            .arg(&cache)
            .arg("--target-dir")
            .arg(temp.path().join("target"))
            .args(["--ignore-filename-regex", "tests/"])
            .args(extra)
            .output()
            .unwrap()
    };
    let output = run(&["--lcov", "--fail-under-lines", "0"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report =
        std::fs::read_to_string(temp.path().join("target/coverage/report/lcov.info")).unwrap();
    assert!(report.contains("src/lib.rs"), "{report}");
    assert!(
        !report.contains("tests/sample.rs") && !report.contains("dependency/src/lib.rs"),
        "{report}"
    );
    let output = run(&["--summary-only", "--fail-under-lines", "101"]);
    assert!(!output.status.success(), "threshold above 100 must fail");
    assert!(String::from_utf8_lossy(&output.stderr).contains("below threshold"));

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let fake_cov = temp.path().join("failing-llvm-cov");
        std::fs::write(
            &fake_cov,
            "#!/bin/sh\nif [ \"$1\" = report ]; then exit 0; fi\nexit 42\n",
        )
        .unwrap();
        std::fs::set_permissions(&fake_cov, std::fs::Permissions::from_mode(0o755)).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_mitts"))
            .current_dir(&root)
            .args(["llvm-cov", "--offline", "--test", "sample", "--cache-dir"])
            .arg(&cache)
            .arg("--target-dir")
            .arg(temp.path().join("target"))
            .args(["--summary-only", "--fail-under-lines", "0"])
            .env("MITTS_LLVM_COV", fake_cov)
            .output()
            .unwrap();
        assert!(
            !output.status.success(),
            "failed LLVM export must fail the coverage gate"
        );
        assert!(String::from_utf8_lossy(&output.stderr).contains("unable to calculate"));
    }
}
