/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Cargo shows an identical diagnostic once per build and counts the repeats: a crate
//! compiled twice (for the host as a proc-macro dependency and for the target, as pest's
//! library is) yields the same warnings twice. Mitts printed every compilation's copy.
//! Each located diagnostic block of a compiler's output is hashed per build; a repeat is
//! dropped and the compiler's own summary line gains "(N duplicates)".

use std::collections::HashSet;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Mutex;

static SEEN: Mutex<Option<HashSet<u64>>> = Mutex::new(None);

/// Forgets the diagnostics shown so far; called when a build starts.
pub fn reset() {
    *SEEN.lock().unwrap_or_else(|e| e.into_inner()) = Some(HashSet::new());
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for d in chars.by_ref() {
                    if d.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

fn is_header(line: &str) -> bool {
    let s = strip_ansi(line);
    s.starts_with("warning") || s.starts_with("error")
}

fn is_summary(block: &str) -> bool {
    let s = strip_ansi(block);
    let first = s.lines().next().unwrap_or("");
    first.starts_with("warning:") && first.trim_end().ends_with("emitted")
}

/// Removes located diagnostics already shown in this build and annotates the summary.
pub fn dedupe(text: &str) -> String {
    let mut blocks: Vec<String> = Vec::new();
    for line in text.split_inclusive('\n') {
        if is_header(line) || blocks.is_empty() {
            blocks.push(line.to_string());
        } else {
            blocks.last_mut().unwrap().push_str(line);
        }
    }
    let mut guard = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    let seen = guard.get_or_insert_with(HashSet::new);
    let mut dropped = 0usize;
    let mut out = String::with_capacity(text.len());
    for block in blocks {
        if block.contains("-->") {
            let mut h = DefaultHasher::new();
            strip_ansi(&block).hash(&mut h);
            if !seen.insert(h.finish()) {
                dropped += 1;
                continue;
            }
            out.push_str(&block);
        } else if dropped > 0 && is_summary(&block) {
            let (first, rest) = block.split_once('\n').unwrap_or((block.as_str(), ""));
            out.push_str(first);
            out.push_str(&format!(" ({dropped} duplicates)"));
            if !rest.is_empty() || block.ends_with('\n') {
                out.push('\n');
                out.push_str(rest);
            }
        } else {
            out.push_str(&block);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{dedupe, reset};

    const WARNING: &str = "warning: unused variable: `x`\n --> src/lib.rs:1:9\n  |\n1 | fn f(x: u32) {}\n  |      ^ help: prefix with an underscore\n  |\n  = note: `#[warn(unused_variables)]` on by default\n\n";

    #[test]
    fn a_repeated_diagnostic_is_shown_once_and_counted() {
        reset();
        let first = dedupe(&format!("{WARNING}warning: 1 warning emitted\n"));
        assert_eq!(first.matches("unused variable").count(), 1);
        assert!(first.contains("warning: 1 warning emitted\n"));
        let second = dedupe(&format!("{WARNING}warning: 1 warning emitted\n"));
        assert_eq!(second.matches("unused variable").count(), 0);
        assert!(second.contains("warning: 1 warning emitted (1 duplicates)\n"));
        let other = dedupe("error[E0425]: cannot find value `y`\n --> src/lib.rs:2:5\n  |\n2 |     y\n  |     ^\n\nerror: aborting due to 1 previous error\n");
        assert!(other.contains("cannot find value") && other.contains("aborting"));
    }
}
