/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Cargo-shaped JSON records for `mitts check|clippy --message-format=json`.
//!
//! rustc is run with `--error-format=json`, so the output of a unit is a stream of JSON lines.
//! Each diagnostic line is wrapped with the unit that produced it:
//!
//! ```text
//! {"reason":"compiler-message","package_id":..,"manifest_path":..,"target":{..},"message":<rustc>}
//! ```
//!
//! Any other line, such as text a compiler wrapper printed, is passed through unchanged for
//! the CLI to send to stderr.

use super::types::PlannedCompilationUnit;
use serde_json::{json, Value};

/// The `target` object of a record. `kind` and `crate_types` are the unit's crate-type list.
fn target_json(unit: &PlannedCompilationUnit) -> Value {
    let kinds: Vec<&str> = unit
        .kind
        .split(',')
        .map(str::trim)
        .filter(|kind| !kind.is_empty())
        .collect();
    json!({
        "kind": kinds,
        "crate_types": kinds,
        "name": unit.target_name,
        "src_path": unit.src_path,
        "edition": unit.edition.as_deref().unwrap_or("2015"),
    })
}

/// A diagnostic is a JSON object carrying rustc's `message`, `level` and `spans`.
fn is_diagnostic(value: &Value) -> bool {
    value.get("$message_type").and_then(Value::as_str) == Some("diagnostic")
        || (value.get("message").is_some()
            && value.get("level").is_some()
            && value.get("spans").is_some())
}

/// Wraps every diagnostic line of `output` in a `compiler-message` record for `unit`. Other
/// lines are kept as they are. The result has one line per record or passed-through line.
pub fn wrap(unit: &PlannedCompilationUnit, output: &str) -> String {
    let target = target_json(unit);
    let mut lines = String::new();
    for line in output.lines().filter(|line| !line.trim().is_empty()) {
        match serde_json::from_str::<Value>(line) {
            Ok(message) if is_diagnostic(&message) => {
                // Written by hand so `reason` comes first, in Cargo's key order; a map would
                // sort the keys alphabetically.
                lines.push_str(&format!(
                    "{{\"reason\":\"compiler-message\",\"package_id\":{},\"manifest_path\":{},\"target\":{},\"message\":{}}}",
                    json!(unit.package_id),
                    json!(unit.manifest_path),
                    target,
                    message,
                ));
            }
            _ => lines.push_str(line),
        }
        lines.push('\n');
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn unit() -> PlannedCompilationUnit {
        PlannedCompilationUnit {
            package_id: "demo@0.1.0".to_string(),
            package_name: "demo".to_string(),
            version: "0.1.0".to_string(),
            target_name: "demo".to_string(),
            kind: "lib".to_string(),
            src_path: PathBuf::from("/w/demo/src/lib.rs"),
            manifest_path: PathBuf::from("/w/demo/Cargo.toml"),
            edition: Some("2021".to_string()),
            ..PlannedCompilationUnit::default()
        }
    }

    const DIAGNOSTIC: &str = r#"{"$message_type":"diagnostic","message":"mismatched types","code":{"code":"E0308","explanation":null},"level":"error","spans":[{"file_name":"src/lib.rs","line_start":1}],"children":[],"rendered":"error[E0308]: mismatched types\n"}"#;

    #[test]
    fn a_diagnostic_becomes_a_cargo_shaped_record() {
        let wrapped = wrap(&unit(), DIAGNOSTIC);
        let record: Value = serde_json::from_str(wrapped.trim_end()).unwrap();
        assert_eq!(record["reason"], "compiler-message");
        assert_eq!(record["package_id"], "demo@0.1.0");
        assert_eq!(record["manifest_path"], "/w/demo/Cargo.toml");
        assert_eq!(record["target"]["name"], "demo");
        assert_eq!(record["target"]["kind"], json!(["lib"]));
        assert_eq!(record["target"]["edition"], "2021");
        assert_eq!(record["message"]["level"], "error");
        assert_eq!(record["message"]["code"]["code"], "E0308");
        assert!(record["message"]["rendered"]
            .as_str()
            .unwrap()
            .contains("E0308"));
        assert_eq!(wrapped.lines().count(), 1, "one record per line");
        assert!(
            wrapped.starts_with("{\"reason\":\"compiler-message\",\"package_id\":"),
            "Cargo's key order, reason first: {wrapped}"
        );
    }

    #[test]
    fn other_lines_pass_through_and_blank_lines_are_dropped() {
        let output = format!("wrapper says hi\n\n{DIAGNOSTIC}\n{{\"not\":\"a diagnostic\"}}\n");
        let wrapped = wrap(&unit(), &output);
        let lines: Vec<&str> = wrapped.lines().collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], "wrapper says hi");
        assert!(lines[1].starts_with("{\"reason\":\"compiler-message\""));
        assert_eq!(lines[2], "{\"not\":\"a diagnostic\"}");
    }

    #[test]
    fn a_multi_crate_type_unit_lists_every_type() {
        let mut multi = unit();
        multi.kind = "staticlib,cdylib,rlib".to_string();
        let record: Value = serde_json::from_str(wrap(&multi, DIAGNOSTIC).trim_end()).unwrap();
        assert_eq!(
            record["target"]["crate_types"],
            json!(["staticlib", "cdylib", "rlib"])
        );
    }

    #[test]
    fn empty_output_stays_empty() {
        assert_eq!(wrap(&unit(), ""), "");
        assert_eq!(wrap(&unit(), "\n\n"), "");
    }
}
