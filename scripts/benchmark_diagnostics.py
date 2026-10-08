# Mitts Build — Rust-first build and package tooling with shared artifact cache
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0

import collections
import re
from pathlib import Path

ANSI_REGEX = re.compile(r'\x1B(?:[@-Z\\-_]|\[[0-?]*[ -/]*[@-~])')
DIAG_HEADER_RE = re.compile(r'^(warning|error)(?:\[([^\]]+)\])?:\s+(.*)$')
LOC_RE = re.compile(r'^\s+-->\s+([^:]+):(\d+):(\d+)')
SUMMARY_RE = re.compile(r'^(warning|error):\s+.*(generated \d+ warning|could not compile|aborting due to)')
MITTS_BANNER_RE = re.compile(r'^\[mitts::.*\]')


def extract_diagnostics_counter(stderr_text: str, ws_root: Path) -> collections.Counter:
    """
    Parses compiler and clippy output into a normalized multiset (Counter) of diagnostics:
    (level, code, rel_file, line, col, normalized_msg).
    Excludes toolchain summary lines and normalizes paths relative to ws_root.
    """
    counter = collections.Counter()
    clean = ANSI_REGEX.sub('', stderr_text)
    lines = clean.splitlines()

    i = 0
    n = len(lines)
    while i < n:
        line = lines[i].strip()
        if not line or MITTS_BANNER_RE.match(line) or SUMMARY_RE.match(line):
            i += 1
            continue

        m_diag = DIAG_HEADER_RE.match(line)
        if m_diag:
            level = m_diag.group(1)
            code = (m_diag.group(2) or "").strip()
            msg = m_diag.group(3).strip()

            rel_file = ""
            line_num = 0
            col_num = 0

            # Scan forward for the first '--> file:line:col' before the next diagnostic header
            j = i + 1
            while j < n and j < i + 12:
                next_line = lines[j].strip()
                if DIAG_HEADER_RE.match(next_line):
                    break
                m_loc = LOC_RE.match(lines[j])
                if m_loc:
                    raw_file_str = m_loc.group(1).strip()
                    try:
                        raw_path = Path(raw_file_str)
                        if raw_path.is_absolute() and raw_path.is_relative_to(ws_root):
                            rel_file = str(raw_path.relative_to(ws_root))
                        else:
                            rel_file = raw_file_str
                    except Exception:
                        rel_file = raw_file_str
                    line_num = int(m_loc.group(2))
                    col_num = int(m_loc.group(3))
                    break
                j += 1

            counter[(level, code, rel_file, line_num, col_num, msg)] += 1
        i += 1

    return counter


def verify_diagnostics_match(cargo_stderr: str, mitts_stderr: str, ws_root: Path) -> tuple[bool, str]:
    """Verifies that Cargo and Mitts emit an identical multiset of diagnostics."""
    cargo_c = extract_diagnostics_counter(cargo_stderr, ws_root)
    mitts_c = extract_diagnostics_counter(mitts_stderr, ws_root)

    if cargo_c == mitts_c:
        total = sum(cargo_c.values())
        return True, f"100% matched {total} diagnostics exactly"

    diff_cargo = cargo_c - mitts_c
    diff_mitts = mitts_c - cargo_c
    msg = (f"Diagnostics mismatch: Cargo has {sum(diff_cargo.values())} extra, "
           f"Mitts has {sum(diff_mitts.values())} extra (Cargo total: {sum(cargo_c.values())}, Mitts total: {sum(mitts_c.values())})")
    return False, msg


def verify_warning_replay(initial_stderr: str, cached_stderr: str, ws_root: Path, is_anomaly_wt: bool) -> tuple[bool, str]:
    """
    Verifies that warnings were accurately replayed on a cache hit.
    If is_anomaly_wt is True, asserts that diagnostics were NOT empty.
    """
    c_init = extract_diagnostics_counter(initial_stderr, ws_root)
    c_cached = extract_diagnostics_counter(cached_stderr, ws_root)

    if is_anomaly_wt and sum(c_init.values()) == 0:
        return False, "Anomaly worktree failed to produce any diagnostics in initial run"

    if c_init == c_cached:
        return True, f"Replayed all {sum(c_init.values())} diagnostics exactly"

    return False, f"Replay mismatch: initial had {sum(c_init.values())}, cached run had {sum(c_cached.values())}"
