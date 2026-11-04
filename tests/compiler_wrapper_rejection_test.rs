/*
 * Mitts Build — compiler wrapper rejection regression
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::process::Command;

#[test]
#[cfg(unix)]
fn compiler_wrappers_run_for_units_and_build_scripts_without_stale_cache() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join(".cargo")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname='wrapper-test'\nversion='0.1.0'\nedition='2021'\n",
    )
    .unwrap();
    std::fs::write(
        root.join("src/lib.rs"),
        "include!(concat!(env!(\"OUT_DIR\"), \"/generated.rs\"));\n",
    )
    .unwrap();
    std::fs::write(root.join("build.rs"), "fn main() { std::fs::write(std::path::Path::new(&std::env::var(\"OUT_DIR\").unwrap()).join(\"generated.rs\"), \"pub fn generated() {}\").unwrap(); }\n").unwrap();
    let log = temp.path().join("wrapper.log");
    let wrapper_path = root.join("wrap.sh");
    std::fs::write(
        &wrapper_path,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nexec \"$@\"\n",
            log.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&wrapper_path, std::fs::Permissions::from_mode(0o755)).unwrap();
    let run = |config: Option<&str>, wrapper: Option<&str>| {
        std::fs::write(root.join(".cargo/config.toml"), config.unwrap_or("")).unwrap();
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mitts"));
        cmd.current_dir(&root)
            .args(["check", "--offline", "--cache-dir"])
            .arg(temp.path().join("cache"))
            .arg("--target-dir")
            .arg(temp.path().join("target"))
            .env("CARGO_HOME", temp.path().join("cargo-home"))
            .env_remove("RUSTC_WRAPPER")
            .env_remove("RUSTC_WORKSPACE_WRAPPER")
            .env_remove("CARGO_BUILD_RUSTC_WRAPPER")
            .env_remove("CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER");
        if let Some(value) = wrapper {
            cmd.env("RUSTC_WRAPPER", value);
        }
        cmd.output().unwrap()
    };
    let initial = run(None, None);
    assert!(
        initial.status.success(),
        "{}",
        String::from_utf8_lossy(&initial.stderr)
    );
    let wrapped = run(Some("[build]\nrustc-wrapper='./wrap.sh'\n"), None);
    assert!(
        wrapped.status.success(),
        "{}",
        String::from_utf8_lossy(&wrapped.stderr)
    );
    let lines = std::fs::read_to_string(&log).unwrap();
    assert!(
        lines.contains("build_script_build") && lines.contains("wrapper_test"),
        "{lines}"
    );
    let before = lines.lines().count();
    let wrapped = run(None, Some(wrapper_path.to_str().unwrap()));
    assert!(
        wrapped.status.success(),
        "{}",
        String::from_utf8_lossy(&wrapped.stderr)
    );
    assert!(std::fs::read_to_string(&log).unwrap().lines().count() > before);
    let inner = root.join("inner.sh");
    std::fs::write(
        &inner,
        format!(
            "#!/bin/sh\nprintf 'inner %s\\n' \"$*\" >> '{}'\nexec \"$@\"\n",
            log.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&inner, std::fs::Permissions::from_mode(0o755)).unwrap();
    let nested = run(
        Some("[build]\nrustc-wrapper='./wrap.sh'\nrustc-workspace-wrapper='./inner.sh'\n"),
        None,
    );
    assert!(
        nested.status.success(),
        "{}",
        String::from_utf8_lossy(&nested.stderr)
    );
    let lines = std::fs::read_to_string(&log).unwrap();
    assert!(
        lines.contains("inner.sh") && lines.contains("inner "),
        "{lines}"
    );
    assert!(run(Some("[build]\nrustc-wrapper='./wrap.sh'\n"), Some(""))
        .status
        .success());
}

#[test]
#[cfg(unix)]
fn wrapper_outputs_do_not_replace_shared_cached_artifacts() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname='wrapper-isolation'\nversion='0.1.0'\nedition='2021'\n",
    )
    .unwrap();
    std::fs::write(root.join("src/lib.rs"), "#[cfg(wrapped)] pub fn value() -> u8 { 2 }\n#[cfg(not(wrapped))] pub fn value() -> u8 { 1 }\n").unwrap();
    std::fs::write(
        root.join("src/main.rs"),
        "fn main() { println!(\"{}\", wrapper_isolation::value()); }\n",
    )
    .unwrap();
    let wrapper = temp.path().join("wrapper.sh");
    std::fs::write(
        &wrapper,
        "#!/bin/sh\ncompiler=$1\nshift\nexec \"$compiler\" \"$@\" --cfg wrapped\n",
    )
    .unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let target = temp.path().join("target");
    let build = |wrapped: bool| {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mitts"));
        cmd.current_dir(&root)
            .args(["build", "--offline", "--cache-dir"])
            .arg(temp.path().join("cache"))
            .arg("--target-dir")
            .arg(&target)
            .env("CARGO_HOME", temp.path().join("cargo-home"))
            .env_remove("RUSTC_WRAPPER")
            .env_remove("RUSTC_WORKSPACE_WRAPPER")
            .env_remove("CARGO_BUILD_RUSTC_WRAPPER")
            .env_remove("CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER");
        if wrapped {
            cmd.env("RUSTC_WRAPPER", &wrapper);
        }
        let output = cmd.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let run = Command::new(target.join("debug/wrapper-isolation"))
            .output()
            .unwrap();
        String::from_utf8(run.stdout).unwrap()
    };
    assert_eq!(build(false).trim(), "1");
    assert_eq!(build(true).trim(), "2");
    assert_eq!(build(false).trim(), "1");
}
