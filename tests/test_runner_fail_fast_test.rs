/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::cli::test::TestArgs;
use mitts::cli::test_runner::{execute_test_binaries, DiscoveredBinaries};
use std::collections::HashMap;
use std::fs;
use std::time::Instant;

#[test]
#[cfg(unix)]
fn test_fail_fast_breaks_by_default_and_runs_all_with_no_fail_fast() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().unwrap();
    let bin_dir = temp.path();

    let marker_file = bin_dir.join("second_test_ran.txt");

    // Binary 1: always fails with exit 101
    let fail_script = bin_dir.join("test_failing.sh");
    fs::write(&fail_script, "#!/bin/sh\nexit 101\n").unwrap();
    let mut perms = fs::metadata(&fail_script).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&fail_script, perms).unwrap();

    // Binary 2: creates marker file and exits 0
    let pass_script = bin_dir.join("test_passing.sh");
    let script_content = format!("#!/bin/sh\ntouch \"{}\"\nexit 0\n", marker_file.display());
    fs::write(&pass_script, script_content).unwrap();
    let mut perms2 = fs::metadata(&pass_script).unwrap().permissions();
    perms2.set_mode(0o755);
    fs::set_permissions(&pass_script, perms2).unwrap();

    // Case 1: Default (fail-fast active, no_fail_fast = false)
    let binaries_default = DiscoveredBinaries {
        test_binaries: vec![
            (
                "failing_test".to_string(),
                fail_script.clone(),
                HashMap::new(),
            ),
            (
                "passing_test".to_string(),
                pass_script.clone(),
                HashMap::new(),
            ),
        ],
        cli_binaries: HashMap::new(),
    };
    let args_default = TestArgs {
        no_fail_fast: false,
        ..Default::default()
    };

    let code_default = execute_test_binaries(
        binaries_default,
        2,
        &args_default,
        bin_dir,
        Instant::now(),
        "test",
    );
    assert_ne!(code_default, 0, "Test runner must report failure");
    assert!(
        !marker_file.exists(),
        "Second test MUST NOT execute when fail-fast is active"
    );

    // Case 2: --no-fail-fast enabled
    let binaries_no_ff = DiscoveredBinaries {
        test_binaries: vec![
            (
                "failing_test".to_string(),
                fail_script.clone(),
                HashMap::new(),
            ),
            (
                "passing_test".to_string(),
                pass_script.clone(),
                HashMap::new(),
            ),
        ],
        cli_binaries: HashMap::new(),
    };
    let args_no_ff = TestArgs {
        no_fail_fast: true,
        ..Default::default()
    };

    let code_no_ff = execute_test_binaries(
        binaries_no_ff,
        2,
        &args_no_ff,
        bin_dir,
        Instant::now(),
        "test",
    );
    assert_ne!(code_no_ff, 0, "Test runner must report failure");
    assert!(
        marker_file.exists(),
        "Second test MUST execute when --no-fail-fast is enabled"
    );
}
