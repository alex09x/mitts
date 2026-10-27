#!/usr/bin/env python3
# Mitts Build — Rust-first build and package tooling with shared artifact cache
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0

"""
Compiler and Clippy diagnostic multiset extraction, parity verification,
cached warning replay verification, and injected anomaly verification.
"""

from __future__ import annotations

import collections
import re
from pathlib import Path

ANSI_REGEX = re.compile(r"\x1B(?:[@-Z\\-_]|\[[0-?]*[ -/]*[@-~])")
DIAG_HEADER_RE = re.compile(r"^(warning|error)(?:\[([^\]]+)\])?:\s+(.*)$")
SHORT_DIAG_RE = re.compile(r"^([^:\s]+):(\d+):(\d+):\s+(warning|error)(?:\[([^\]]+)\])?:\s+(.*)$")
LOC_RE = re.compile(r"^\s+-->\s+([^:]+):(\d+):(\d+)")
SUMMARY_RE = re.compile(
    r"^(warning|error):\s+.*(generated \d+ warning|could not compile|aborting due to|\d+ warnings? emitted)"
)
MITTS_BANNER_RE = re.compile(r"^\[mitts::.*\]")


def _rel_file(raw_file_str: str, ws_root: Path) -> str:
    try:
        raw_path = Path(raw_file_str)
        if raw_path.is_absolute():
            resolved_ws = ws_root.resolve()
            resolved_p = raw_path.resolve()
            if str(resolved_p).startswith(str(resolved_ws) + "/"):
                return str(resolved_p.relative_to(resolved_ws))
        return raw_file_str
    except Exception:
        return raw_file_str


def extract_diagnostics_counter(stderr_text: str, ws_root: Path) -> collections.Counter:
    """
    Parses compiler and clippy output into a normalized multiset (Counter) of diagnostics:
    (level, code, rel_file, line, col, normalized_msg).
    """
    counter: collections.Counter = collections.Counter()
    clean = ANSI_REGEX.sub("", stderr_text)
    lines = clean.splitlines()

    i = 0
    n = len(lines)
    while i < n:
        line = lines[i].strip()
        if not line or MITTS_BANNER_RE.match(line) or SUMMARY_RE.match(line):
            i += 1
            continue

        m_short = SHORT_DIAG_RE.match(line)
        if m_short:
            rel_f = _rel_file(m_short.group(1).strip(), ws_root)
            line_num = int(m_short.group(2))
            col_num = int(m_short.group(3))
            level = m_short.group(4)
            code = (m_short.group(5) or "").strip()
            msg = m_short.group(6).strip()
            if ": help:" in msg:
                msg = msg.split(": help:", 1)[0].strip()
            counter[(level, code, rel_f, line_num, col_num, msg)] += 1
            i += 1
            continue

        m_diag = DIAG_HEADER_RE.match(line)
        if m_diag:
            level = m_diag.group(1)
            code = (m_diag.group(2) or "").strip()
            msg = m_diag.group(3).strip()
            rel_f = ""
            line_num = 0
            col_num = 0

            j = i + 1
            while j < n and j < i + 12:
                next_line = lines[j].strip()
                if DIAG_HEADER_RE.match(next_line):
                    break
                m_loc = LOC_RE.match(lines[j])
                if m_loc:
                    rel_f = _rel_file(m_loc.group(1).strip(), ws_root)
                    line_num = int(m_loc.group(2))
                    col_num = int(m_loc.group(3))
                    break
                j += 1

            # A header without a source location is the tool talking, not the compiler:
            # Cargo's `unused manifest key`, `virtual workspace defaulting to resolver 1`,
            # profile notes and the like. Parity is about compiler diagnostics.
            if not rel_f:
                i += 1
                continue
            counter[(level, code, rel_f, line_num, col_num, msg)] += 1
        i += 1

    return counter


def verify_anomaly_present(stderr_text: str, ws_root: Path, anomaly_info: dict | None) -> bool:
    """Checks that the injected anomaly (`unused_test_var` at expected file:line) is reported."""
    if not anomaly_info:
        return False
    expected_file = anomaly_info.get("relative_file", "")
    expected_line = anomaly_info.get("line")
    expected_sym = anomaly_info.get("expected_symbol", "unused_test_var")
    counter = extract_diagnostics_counter(stderr_text, ws_root)
    for (_lvl, _code, rel_f, line_num, _col, msg) in counter:
        if expected_sym in msg and (not expected_file or rel_f.endswith(expected_file)):
            if expected_line is None or line_num == expected_line:
                return True
    return expected_sym in stderr_text


def verify_diagnostics_match(cargo_stderr: str, mitts_stderr: str, ws_root: Path) -> tuple[bool, str]:
    """Verifies that Cargo and Mitts emit an identical multiset of diagnostics."""
    cargo_c = extract_diagnostics_counter(cargo_stderr, ws_root)
    mitts_c = extract_diagnostics_counter(mitts_stderr, ws_root)

    if cargo_c == mitts_c:
        total = sum(cargo_c.values())
        return True, f"100% matched {total} diagnostics exactly"

    diff_cargo = cargo_c - mitts_c
    diff_mitts = mitts_c - cargo_c
    msg = (
        f"Diagnostics mismatch: Cargo has {sum(diff_cargo.values())} extra, "
        f"Mitts has {sum(diff_mitts.values())} extra "
        f"(Cargo total: {sum(cargo_c.values())}, Mitts total: {sum(mitts_c.values())})"
    )
    return False, msg


def verify_warning_replay(
    initial_stderr: str, cached_stderr: str, ws_root: Path, is_anomaly_wt: bool
) -> tuple[bool, str]:
    """Verifies that warnings were accurately replayed on a cache hit."""
    c_init = extract_diagnostics_counter(initial_stderr, ws_root)
    c_cached = extract_diagnostics_counter(cached_stderr, ws_root)

    if is_anomaly_wt and sum(c_init.values()) == 0:
        return False, "Anomaly worktree failed to produce any diagnostics in initial run"

    if c_init == c_cached:
        return True, f"Replayed all {sum(c_init.values())} diagnostics exactly"

    return (
        False,
        f"Replay mismatch: initial had {sum(c_init.values())}, cached run had {sum(c_cached.values())}",
    )
