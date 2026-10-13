#
# Mitts Build — Rust-first build and package tooling with shared artifact cache
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0
#

"""
Output parsers and small helpers for the T5 build-swarm audit.

Everything here is pure: text in, dict out. Kept apart from the runner so the
parsing rules can be read and tested on their own.
"""

import os
import re
from pathlib import Path

DEFECT_TEST_NAME = "__intentional_test_failure"


def inject_test_defect(wt_dir: Path):
    """Appends one always-failing #[test] to the first integration test file found."""
    candidates = []
    skip_parts = {
        ".git", "target", "plugins", "plugin_persistence", "tests-build",
        "tests-integration", "benches", "examples", "stress-test",
    }
    for r, _, files in os.walk(wt_dir):
        p_r = Path(r)
        if skip_parts.intersection(p_r.parts):
            continue
        if "tests" in p_r.parts:
            for f in sorted(files):
                if f.endswith(".rs") and not f.startswith((".", "_")) and f not in ("lib.rs", "main.rs"):
                    candidates.append(p_r / f)
    if not candidates:
        for r, _, files in os.walk(wt_dir):
            if ".git" in r or "target" in r:
                continue
            for f in sorted(files):
                if f.endswith(".rs") and not f.startswith("."):
                    candidates.append(Path(r) / f)
                    break
            if candidates:
                break
    target = candidates[0] if candidates else None
    if not target:
        return None
    snippet = (
        f"\n#[test]\nfn {DEFECT_TEST_NAME}() {{\n"
        "    assert_eq!(2 + 2, 5, \"intentional test failure for audit\");\n}\n"
    )
    with open(target, "r") as f:
        content = f.read()
    with open(target, "w") as f:
        f.write(content + snippet)
    return str(target.relative_to(wt_dir))


def parse_test_results(output: str):
    """Sums `test result:` lines of every executed test binary."""
    matches = re.findall(
        r"test result: (ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored",
        output,
    )
    return {
        "binaries_executed": len(matches),
        "passed": sum(int(m[1]) for m in matches),
        "failed": sum(int(m[2]) for m in matches),
        "ignored": sum(int(m[3]) for m in matches),
    }


def parse_build_output(output: str, tool: str):
    """
    Number of test binaries produced by a `test --no-run` invocation.

    cargo prints one `Executable <target> (<path>)` line per binary; Mitts prints
    `Compiled N test binaries (--no-run)`. The cargo target list is returned too so
    one worktree per series can keep the names for the reader.
    """
    if tool == "mitts_cas":
        m = re.search(r"Compiled (\d+) test binaries \(--no-run\)", output)
        return {"test_binaries_count": int(m.group(1)) if m else 0, "targets": []}
    execs = re.findall(r"Executable\s+(.+?)\s+\((.+?)\)", output)
    targets = sorted(e[0].strip() for e in execs)
    res = {"test_binaries_count": len(execs), "targets": targets}
    m_fin = re.search(
        r"Finished\s+`?(?:test|dev|release)`?\s+profile.*?in\s+(?:(\d+)m\s+)?([\d\.]+)s",
        output,
    )
    if m_fin:
        mins = float(m_fin.group(1)) if m_fin.group(1) else 0.0
        res["cargo_reported_s"] = round(mins * 60.0 + float(m_fin.group(2)), 3)
    return res


def parse_mitts_stats(text: str):
    m = re.search(
        r"\[mitts::stats\]\s+compiled=(\d+)\s+cached=(\d+)\s+total=(\d+)(?:\s+fast_lock=(\d+))?",
        text,
    )
    if not m:
        return {"compiled": 0, "cached": 0, "total": 0, "fast_lock": 0}
    return {
        "compiled": int(m.group(1)),
        "cached": int(m.group(2)),
        "total": int(m.group(3)),
        "fast_lock": int(m.group(4)) if m.group(4) is not None else 0,
    }


def parse_mitts_io(text: str):
    """MITTS_TIMINGS=1 diagnostics: zero-copy I/O counters and per-phase sums."""
    res = {}
    m_io = re.search(
        r"\[mitts::io\]\s+bytes_cloned=(\d+)\s+bytes_copied=(\d+)\s+bytes_hardlinked=(\d+)"
        r"\s+files_cloned=(\d+)\s+files_copied=(\d+)\s+files_hardlinked=(\d+)",
        text,
    )
    if m_io:
        keys = ["bytes_cloned", "bytes_copied", "bytes_hardlinked",
                "files_cloned", "files_copied", "files_hardlinked"]
        res["io_stats"] = {k: int(m_io.group(i + 1)) for i, k in enumerate(keys)}
    m_sum = re.search(
        r"\[mitts::timings_sum\]\s+restore_sum_s=([\d\.]+)\s+restore_miss_sum_s=([\d\.]+)"
        r"\s+publish_sum_s=([\d\.]+)\s+rustc_sum_s=([\d\.]+)",
        text,
    )
    if m_sum:
        keys = ["restore_sum_s", "restore_miss_sum_s", "publish_sum_s", "rustc_sum_s"]
        res["timings_sum_s"] = {k: float(m_sum.group(i + 1)) for i, k in enumerate(keys)}
    return res


def parse_sccache_stats(text: str):
    def get_num(key):
        m = re.search(rf"{key}\s+(\d+)", text)
        return int(m.group(1)) if m else 0

    return {
        "compile_requests": get_num("Compile requests"),
        "compile_requests_executed": get_num("Compile requests executed"),
        "cache_hits": get_num("Cache hits"),
        "cache_misses": get_num("Cache misses"),
    }


def extract_failed_tests(text: str):
    failed = []
    for line in text.splitlines():
        line_s = line.strip()
        if line_s.startswith("test ") and line_s.endswith(" ... FAILED"):
            name = line_s[5:-11].strip()
            if name and name not in failed:
                failed.append(name)
    return failed


def compute_stats(vals):
    if not vals:
        return {"median": 0.0, "q1": 0.0, "q3": 0.0, "iqr": 0.0, "min": 0.0, "max": 0.0}
    s = sorted(vals)
    n = len(s)
    median = s[n // 2] if n % 2 == 1 else (s[n // 2 - 1] + s[n // 2]) / 2.0
    q1 = s[n // 4]
    q3 = s[(3 * n) // 4]
    return {
        "median": round(median, 3),
        "q1": round(q1, 3),
        "q3": round(q3, 3),
        "iqr": round(q3 - q1, 3),
        "min": round(s[0], 3),
        "max": round(s[-1], 3),
    }


def spread_pct(vals):
    """(max - min) / median in percent; 0 when the series is empty or has zeros."""
    if not vals or min(vals) <= 0:
        return 0.0
    med = compute_stats(vals)["median"]
    return round(((max(vals) - min(vals)) / med) * 100.0, 2)
