/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Cargo emits `[lints]` flags in ascending priority, then by name, so a lint group set
//! at a negative priority comes before the lints a manifest allows at priority 0 and the
//! allows win. hyper warns `clippy::restriction` at priority -2 and allows a long list of
//! its lints; emitted in table order the group came last and re-enabled all of them.

use mitts::adapter::rust::manifest_inherit::extract_lint_flags_from_lints;

#[test]
fn groups_with_lower_priority_come_before_the_allows_that_override_them() {
    let lints: toml::Value = toml::from_str(
        r#"
[rust]
unused = "warn"

[clippy]
pedantic = { level = "warn", priority = -1 }
restriction = { level = "warn", priority = -2 }
arithmetic_side_effects = "allow"
implicit_return = "allow"
"#,
    )
    .unwrap();
    let flags = extract_lint_flags_from_lints(&lints);
    assert_eq!(
        flags,
        vec![
            "-Wclippy::restriction",
            "-Wclippy::pedantic",
            "-Aclippy::arithmetic_side_effects",
            "-Aclippy::implicit_return",
            "-Wunused",
        ]
    );
}
