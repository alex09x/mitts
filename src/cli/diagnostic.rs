/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use miette::{Diagnostic, NamedSource, SourceSpan};
use std::fs;
use std::path::Path;
use thiserror::Error;

/// Structured compiler diagnostic formatted for miette rendering.
#[derive(Debug, Error)]
#[error("{header}")]
pub struct CompilerDiagnostic {
    pub header: String,
    pub code: Option<String>,
    pub src: Option<NamedSource<String>>,
    pub span: Option<SourceSpan>,
    pub label: Option<String>,
    pub help: Option<String>,
}

impl Diagnostic for CompilerDiagnostic {
    fn code<'a>(&'a self) -> Option<Box<dyn std::fmt::Display + 'a>> {
        self.code
            .as_ref()
            .map(|c| Box::new(c) as Box<dyn std::fmt::Display>)
    }

    fn severity(&self) -> Option<miette::Severity> {
        Some(miette::Severity::Error)
    }

    fn help<'a>(&'a self) -> Option<Box<dyn std::fmt::Display + 'a>> {
        self.help
            .as_ref()
            .map(|h| Box::new(h) as Box<dyn std::fmt::Display>)
    }

    fn source_code(&self) -> Option<&dyn miette::SourceCode> {
        self.src.as_ref().map(|s| s as &dyn miette::SourceCode)
    }

    fn labels(&self) -> Option<Box<dyn Iterator<Item = miette::LabeledSpan> + '_>> {
        if let (Some(span), Some(ref label)) = (self.span, &self.label) {
            Some(Box::new(std::iter::once(miette::LabeledSpan::new(
                Some(label.clone()),
                span.offset(),
                span.len(),
            ))))
        } else {
            None
        }
    }
}

/// Parses raw stderr output and renders structured miette reports.
pub fn render_compiler_diagnostics(stderr: &str, is_verbose: bool) {
    if stderr.trim().is_empty() {
        return;
    }

    let diagnostics = parse_diagnostics(stderr);
    if diagnostics.is_empty() {
        let diag = CompilerDiagnostic {
            header: "Compilation encountered errors".to_string(),
            code: Some("mitts::compiler::failed".to_string()),
            src: None,
            span: None,
            label: None,
            help: Some(stderr.trim().to_string()),
        };
        eprintln!("{:?}", miette::Report::new(diag));
        return;
    }

    for (diag, cmd, cwd) in diagnostics {
        eprintln!("{:?}", miette::Report::new(diag));
        if is_verbose {
            if let Some(c) = cmd {
                eprintln!("  \x1b[2mcommand: {}\x1b[0m", c);
            }
            if let Some(w) = cwd {
                eprintln!("  \x1b[2mcwd: {}\x1b[0m", w);
            }
            eprintln!();
        }
    }
}

/// Parses stderr into individual CompilerDiagnostic entries with command and cwd metadata.
fn parse_diagnostics(stderr: &str) -> Vec<(CompilerDiagnostic, Option<String>, Option<String>)> {
    let mut results = Vec::new();
    let sections = split_unit_sections(stderr);

    for (unit_info, cmd, cwd, body) in sections {
        let (package, version, kind, target) = parse_unit_id(&unit_info);
        let entries = parse_rustc_errors(&body);

        if entries.is_empty() {
            let diag = CompilerDiagnostic {
                header: format!(
                    "Failed to compile {} v{} [{}:{}]",
                    package, version, kind, target
                ),
                code: Some("mitts::rustc::failed".to_string()),
                src: None,
                span: None,
                label: None,
                help: if body.trim().is_empty() {
                    None
                } else {
                    Some(body.trim().to_string())
                },
            };
            results.push((diag, cmd, cwd));
        } else {
            for (code, msg, loc, help) in entries {
                let (src, span, label) = if let Some((file, line, col)) = loc {
                    resolve_source_span(&file, line, col, cwd.as_deref())
                } else {
                    (None, None, None)
                };

                let diag = CompilerDiagnostic {
                    header: format!(
                        "Failed to compile {} v{} [{}:{}]",
                        package, version, kind, target
                    ),
                    code: Some(format!("rustc::{}", code)),
                    src,
                    span,
                    label: label.or(Some(msg.clone())),
                    help: help.or_else(|| if span.is_none() { Some(msg) } else { None }),
                };
                results.push((diag, cmd.clone(), cwd.clone()));
            }
        }
    }

    results
}

fn split_unit_sections(stderr: &str) -> Vec<(String, Option<String>, Option<String>, String)> {
    let mut sections = Vec::new();
    let marker = "rustc failed for unit '";

    if !stderr.contains(marker) {
        sections.push((
            "unknown@0.0.0::lib:unknown".to_string(),
            None,
            None,
            stderr.to_string(),
        ));
        return sections;
    }

    for part in stderr.split(marker) {
        if part.trim().is_empty() {
            continue;
        }
        let Some((unit_id, rest)) = part.split_once('\'') else {
            continue;
        };

        let mut cmd = None;
        let mut cwd = None;
        let mut body_lines = Vec::new();

        for line in rest.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("command:") {
                cmd = Some(
                    trimmed
                        .strip_prefix("command:")
                        .unwrap_or("")
                        .trim()
                        .to_string(),
                );
            } else if trimmed.starts_with("cwd:") {
                cwd = Some(
                    trimmed
                        .strip_prefix("cwd:")
                        .unwrap_or("")
                        .trim()
                        .to_string(),
                );
            } else if !trimmed.starts_with("(exit code") {
                body_lines.push(line);
            }
        }

        sections.push((unit_id.to_string(), cmd, cwd, body_lines.join("\n")));
    }

    sections
}

fn parse_unit_id(unit_id: &str) -> (String, String, String, String) {
    let clean_id = unit_id.strip_suffix(":host").unwrap_or(unit_id);
    let (pkg_ver, target_part) = clean_id.split_once("::").unwrap_or((clean_id, "lib:"));
    let (pkg, ver) = pkg_ver.split_once('@').unwrap_or((pkg_ver, ""));
    let (kind, target) = target_part.split_once(':').unwrap_or((target_part, pkg));
    (
        pkg.to_string(),
        ver.to_string(),
        kind.to_string(),
        target.to_string(),
    )
}

type ParsedRustcError = (
    String,
    String,
    Option<(String, usize, usize)>,
    Option<String>,
);

fn parse_rustc_errors(body: &str) -> Vec<ParsedRustcError> {
    let mut errors = Vec::new();
    let mut cur_code = String::new();
    let mut cur_msg = String::new();
    let mut cur_loc = None;
    let mut cur_help = None;

    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("error[") || trimmed.starts_with("error:") {
            if !cur_code.is_empty() || !cur_msg.is_empty() {
                errors.push((
                    cur_code.clone(),
                    cur_msg.clone(),
                    cur_loc.take(),
                    cur_help.take(),
                ));
            }
            if let Some(rest) = trimmed.strip_prefix("error[") {
                let (code, msg) = rest.split_once("]: ").unwrap_or((rest, ""));
                cur_code = code.to_string();
                cur_msg = msg.to_string();
            } else if let Some(msg) = trimmed.strip_prefix("error: ") {
                cur_code = "error".to_string();
                cur_msg = msg.to_string();
            }
        } else if trimmed.starts_with("--> ") {
            let loc_str = trimmed.strip_prefix("--> ").unwrap_or("").trim();
            if let Some((path, rest)) = loc_str.rsplit_once(':') {
                if let Some((line_s, col_s)) = rest.split_once(':') {
                    if let (Ok(l), Ok(c)) = (line_s.parse::<usize>(), col_s.parse::<usize>()) {
                        cur_loc = Some((path.to_string(), l, c));
                    }
                } else if let Some((p2, line_s)) = path.rsplit_once(':') {
                    if let (Ok(l), Ok(c)) = (line_s.parse::<usize>(), rest.parse::<usize>()) {
                        cur_loc = Some((p2.to_string(), l, c));
                    }
                }
            }
        } else if trimmed.starts_with("= help: ") {
            let h = trimmed.strip_prefix("= help: ").unwrap_or("").trim();
            cur_help = Some(h.to_string());
        }
    }

    if !cur_code.is_empty() || !cur_msg.is_empty() {
        errors.push((cur_code, cur_msg, cur_loc, cur_help));
    }

    errors
}

fn resolve_source_span(
    file_str: &str,
    line_num: usize,
    col_num: usize,
    cwd: Option<&str>,
) -> (
    Option<NamedSource<String>>,
    Option<SourceSpan>,
    Option<String>,
) {
    let p = Path::new(file_str);
    let resolved = if p.is_absolute() {
        p.to_path_buf()
    } else if let Some(base) = cwd {
        Path::new(base).join(p)
    } else {
        p.to_path_buf()
    };

    let Ok(content) = fs::read_to_string(&resolved) else {
        return (None, None, None);
    };

    let mut byte_offset = 0;
    let mut span_len = 1;
    for (idx, l) in content.lines().enumerate() {
        if idx + 1 == line_num {
            let col_offset = col_num.saturating_sub(1);
            byte_offset += col_offset;
            span_len = l.len().saturating_sub(col_offset).max(1);
            break;
        }
        byte_offset += l.len() + 1;
    }

    let span = SourceSpan::new(byte_offset.into(), span_len);
    let src = NamedSource::new(resolved.display().to_string(), content);
    (Some(src), Some(span), None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_rustc_error_with_code_and_location() {
        let stderr = r#"rustc failed for unit 'arrayvec@0.7.6::lib:arrayvec' (exit code 1):
  command: "rustc" "--crate-name" "arrayvec"
  cwd: /tmp/test_dir
error[E0658]: use of unstable library feature rustc_private
 --> src/lib.rs:10:1
  |
10 | extern crate serde;
  | ^^^^^^^^^^^^^^^^^^^
  = help: add `#![feature(rustc_private)]` to the crate attributes to enable
"#;
        let diags = parse_diagnostics(stderr);
        assert_eq!(diags.len(), 1);
        let (diag, cmd, cwd) = &diags[0];
        assert_eq!(
            diag.header,
            "Failed to compile arrayvec v0.7.6 [lib:arrayvec]"
        );
        assert_eq!(diag.code.as_deref(), Some("rustc::E0658"));
        assert_eq!(
            diag.help.as_deref(),
            Some("add `#![feature(rustc_private)]` to the crate attributes to enable")
        );
        assert!(cmd.is_some());
        assert_eq!(cwd.as_deref(), Some("/tmp/test_dir"));
    }

    #[test]
    fn test_parse_macro_compile_error() {
        let stderr = r#"rustc failed for unit 'glam@0.30.9::lib:glam' (exit code 1):
  command: "rustc" "--crate-name" "glam"
  cwd: /tmp/glam
error: You must specify a math backend. Consider enabling either std, libm, or nostd-libm.
 --> src/lib.rs:94:9
  |
94 | compile_error!("backend missing");
  | ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
"#;
        let diags = parse_diagnostics(stderr);
        assert_eq!(diags.len(), 1);
        let (diag, _, _) = &diags[0];
        assert_eq!(diag.header, "Failed to compile glam v0.30.9 [lib:glam]");
        assert_eq!(diag.code.as_deref(), Some("rustc::error"));
    }
}
