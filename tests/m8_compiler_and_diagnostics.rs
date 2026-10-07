/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::rust::build_script::BuildScriptDirectives;
use mitts::cli::diagnostic::render_compiler_diagnostics;
use mitts::cli::Ui;

#[test]
fn test_m8_compiler_diagnostics_rendering() {
    // 1. Empty stderr does nothing
    render_compiler_diagnostics("", false);

    // 2. Generic error fallback
    render_compiler_diagnostics("fatal error: could not compile crate", false);
    render_compiler_diagnostics("fatal error: could not compile crate", true);

    // 3. Rustc error with code, location, and label
    let sample_err = r#"
error[E0425]: cannot find value `xyz` in this scope
 --> src/main.rs:10:5
  |
10|     xyz.do_something();
  |     ^^^ not found in this scope
"#;
    render_compiler_diagnostics(sample_err, false);
    render_compiler_diagnostics(sample_err, true);

    // 4. Macro error
    let macro_err = r#"
error: proc-macro derive panicked
 --> src/lib.rs:5:10
  |
5 | #[derive(Custom)]
  |          ^^^^^^
  |
  = help: message: Custom macro failed
"#;
    render_compiler_diagnostics(macro_err, false);
}

#[test]
fn test_m8_build_script_directives_parsing() {
    let output = r#"
cargo:rustc-link-lib=z
cargo:rustc-link-search=/usr/local/lib
cargo:rustc-cfg=has_feature_x
cargo:rustc-env=VAR=123
cargo:rerun-if-changed=build.rs
cargo:rerun-if-env-changed=TARGET
"#;

    let mut directives = BuildScriptDirectives::parse_stdout(output);
    assert_eq!(directives.link_libs, vec!["z"]);
    assert_eq!(directives.link_searches, vec!["/usr/local/lib"]);
    assert_eq!(directives.cfgs, vec!["has_feature_x"]);
    assert_eq!(directives.envs.get("VAR").map(|s| s.as_str()), Some("123"));

    directives.links = Some("z".to_string());
    directives.metadata.insert("include".to_string(), "/usr/include".to_string());
    let dep_vars = directives.dep_env_vars();
    assert_eq!(dep_vars.get("DEP_Z_INCLUDE").map(|s| s.as_str()), Some("/usr/include"));
}

#[test]
fn test_m8_ui_badges_and_helpers() {
    let b1 = Ui::badge_compile();
    let b2 = Ui::badge_ac_hit();
    let b3 = Ui::badge_cached();
    let b4 = Ui::badge_download();

    assert!(!b1.is_empty());
    assert!(!b2.is_empty());
    assert!(!b3.is_empty());
    assert!(!b4.is_empty());
    let b5 = Ui::badge_failed();
    assert!(!b5.is_empty());

    assert_eq!(Ui::format_bytes(500), "500 B");
    assert_eq!(Ui::format_bytes(1024), "1.0 KB");
    assert_eq!(Ui::format_bytes(1024 * 1024), "1.00 MB");
    assert_eq!(Ui::format_bytes(1024 * 1024 * 1024), "1.00 GB");

    Ui::header("test-header");
    Ui::stage_start("🔧", "Testing UI stage start");
    Ui::stage_step("📦", "Testing UI stage step");
    Ui::stage_item(false, &b1, "Item 1");
    Ui::stage_item(true, &b2, "Item 2");
    Ui::finish("test", "0.01s", "Done");
    Ui::finish_failed("test", "0.02s", "Failed test");

    let err = anyhow::anyhow!("Sample error for reporting");
    Ui::report_error(&err);
    Ui::report_message("Info message", Some("check this help tip"));
    Ui::report_message("Plain message", None);

    let pb = Ui::create_build_progress("build");
    Ui::set_build_progress_total(&pb, 5, 2);
    pb.finish_and_clear();
}
