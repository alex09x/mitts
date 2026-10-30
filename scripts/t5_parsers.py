#!/usr/bin/env python3
# Mitts Build — Rust-first build and package tooling with shared artifact cache
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0

"""
Deterministic target selection via `cargo metadata --offline --no-deps`, defect
injection, and output parsers for Mitts and Cargo across T0-T5 benchmarks.
"""

from __future__ import annotations

import json
import os
import re
import subprocess
from pathlib import Path

DEFECT_TEST_NAME = "__intentional_test_failure"


def has_crate_level_cfg(src: Path) -> bool:
    """A root file starting with `#![cfg(...)]` compiles to nothing on hosts the cfg excludes:
    curve25519-dalek-derive's tests are x86-only, so a defect injected there on an arm64 Mac
    never ran and no tool could catch it. Such a file is no place to inject anything."""
    try:
        with open(src, encoding="utf-8", errors="replace") as fh:
            for _ in range(60):
                line = fh.readline()
                if not line:
                    break
                stripped = line.strip()
                if stripped.startswith("#![cfg("):
                    return True
                if stripped and not stripped.startswith(("#!", "//", "/*", "*")):
                    break
    except OSError:
        pass
    return False


def select_workspace_targets(ws_dir: Path, env: dict[str, str] | None = None) -> dict:
    """
    Uses `cargo metadata --offline --no-deps --format-version 1` to deterministically
    pick compiled workspace member source files (avoiding filesystem-order `os.walk`
    or non-root files like `tests/common/mod.rs` or `tests/ui/*.rs`).
    """
    ws_dir = Path(ws_dir).resolve()
    lib_or_bin_candidates: list[Path] = []
    test_candidates: list[Path] = []
    unharnessed_tests = 0

    try:
        out = subprocess.check_output(
            ["cargo", "metadata", "--offline", "--no-deps", "--format-version", "1"],
            cwd=ws_dir,
            env=env,
            text=True,
            stderr=subprocess.DEVNULL,
        )
        meta = json.loads(out)
        members = set(meta.get("workspace_members", []))
        pkgs = sorted(meta.get("packages", []), key=lambda p: (0 if Path(p["manifest_path"]).parent == ws_dir else 1, p["name"]))
        for pkg in pkgs:
            if members and pkg.get("id") not in members:
                continue
            for tgt in sorted(pkg.get("targets", []), key=lambda t: t.get("name", "")):
                kinds = set(tgt.get("kind", []))
                src_p = Path(tgt.get("src_path", ""))
                harness = tgt.get("harness", True)
                if ("test" in kinds or "bench" in kinds) and not harness:
                    if "test" in kinds:
                        unharnessed_tests += 1
                    continue
                if not src_p.is_file() or has_crate_level_cfg(src_p):
                    continue
                if "lib" in kinds or "rlib" in kinds or "bin" in kinds:
                    lib_or_bin_candidates.append(src_p)
                if "test" in kinds and harness:
                    test_candidates.append(src_p)
    except Exception:
        pass

    if not lib_or_bin_candidates:
        skip_dirs = {".git", "target", "benches", "fuzz", "examples", "tests", "tests-build", "tests-integration"}
        for root, dirs, files in os.walk(ws_dir):
            dirs[:] = sorted(d for d in dirs if d not in skip_dirs and not d.startswith("."))
            for fname in ("lib.rs", "main.rs"):
                if fname in files and not has_crate_level_cfg(Path(root) / fname):
                    lib_or_bin_candidates.append(Path(root) / fname)

    if not test_candidates:
        test_candidates = list(lib_or_bin_candidates)

    lib_src = lib_or_bin_candidates[0] if lib_or_bin_candidates else None
    test_src = test_candidates[0] if test_candidates else lib_src
    return {
        "lib_or_bin_src": lib_src,
        "test_src": test_src,
        "unharnessed_test_targets": unharnessed_tests,
    }


def inject_test_defect(wt_dir: Path, env: dict[str, str] | None = None) -> str:
    """Appends one always-failing #[test] to a verified compiled test target root."""
    wt_dir = Path(wt_dir).resolve()
    info = select_workspace_targets(wt_dir, env=env)
    target = info["test_src"]
    if not target or not target.is_file():
        raise RuntimeError(f"Could not locate a valid test target source file in {wt_dir}")
    snippet = (
        f"\n#[test]\nfn {DEFECT_TEST_NAME}() {{\n"
        '    assert_eq!(2 + 2, 5, "intentional test failure for audit");\n}\n'
    )
    content = target.read_text(encoding="utf-8", errors="replace")
    target.write_text(content + snippet, encoding="utf-8")
    return str(target.relative_to(wt_dir))


def extract_failed_tests(text: str) -> list[str]:
    failed: list[str] = []
    for line in text.splitlines():
        line_s = line.strip()
        if line_s.startswith("test ") and line_s.endswith(" ... FAILED"):
            name = line_s[5:-11].strip()
            if name and name not in failed:
                failed.append(name)
    return failed


def parse_test_results(output: str) -> dict:
    """Sums `test result:` lines and verifies whether DEFECT_TEST_NAME failed."""
    matches = re.findall(
        r"test result: (ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored",
        output,
    )
    failed_tests = extract_failed_tests(output)
    return {
        "binaries_executed": len(matches),
        "passed": sum(int(m[1]) for m in matches),
        "failed": sum(int(m[2]) for m in matches),
        "ignored": sum(int(m[3]) for m in matches),
        "failed_tests": failed_tests,
        "defect_reported": any(DEFECT_TEST_NAME in n for n in failed_tests),
    }


def parse_mitts_stats(text: str) -> dict:
    m = re.search(
        r"\[mitts::stats\]\s+compiled=(\d+)\s+cached=(\d+)\s+(?:tainted=(\d+)\s+)?total=(\d+)(?:\s+fast_lock=(\d+))?",
        text,
    )
    if not m:
        return {"compiled": 0, "cached": 0, "tainted": 0, "total": 0, "fast_lock": 0}
    return {
        "compiled": int(m.group(1)),
        "cached": int(m.group(2)),
        "tainted": int(m.group(3)) if m.group(3) is not None else 0,
        "total": int(m.group(4)),
        "fast_lock": int(m.group(5)) if m.group(5) is not None else 0,
    }


def parse_build_output(output: str, tool: str) -> dict:
    """Extracts test binary counts and unit compilation counts for mitts and cargo."""
    if tool == "mitts_cas":
        m = re.search(r"Compiled (\d+) test binaries(?:\s+\(--no-run\))?", output)
        stats = parse_mitts_stats(output)
        return {
            "test_binaries_count": int(m.group(1)) if m else 0,
            "units_compiled": stats["compiled"],
            "units_cached": stats["cached"],
            "units_tainted": stats["tainted"],
            "units_total": stats["total"],
            "fast_lock": stats["fast_lock"],
            "targets": [],
        }
    execs = re.findall(r"Executable\s+(.+?)\s+\((.+?)\)", output)
    compiled = len(re.findall(r"^\s*Compiling\s+\S+\s+v", output, flags=re.MULTILINE))
    fresh = len(re.findall(r"^\s*Fresh\s+\S+\s+v", output, flags=re.MULTILINE))
    targets = sorted(e[0].strip() for e in execs)
    res = {
        "test_binaries_count": len(execs),
        "units_compiled": compiled,
        "units_cached": fresh,
        "units_total": compiled + fresh,
        "targets": targets,
    }
    m_fin = re.search(
        r"Finished\s+`?(?:test|dev|release)`?\s+profile.*?in\s+(?:(\d+)m\s+)?([\d\.]+)s",
        output,
    )
    if m_fin:
        mins = float(m_fin.group(1)) if m_fin.group(1) else 0.0
        res["cargo_reported_s"] = round(mins * 60.0 + float(m_fin.group(2)), 3)
    return res


def parse_mitts_io(text: str) -> dict:
    """MITTS_TIMINGS=1 diagnostics: zero-copy I/O counters and per-phase sums."""
    res: dict = {}
    m_io = re.search(
        r"\[mitts::io\]\s+bytes_cloned=(\d+)\s+bytes_copied=(\d+)\s+bytes_hardlinked=(\d+)"
        r"\s+files_cloned=(\d+)\s+files_copied=(\d+)\s+files_hardlinked=(\d+)",
        text,
    )
    if m_io:
        keys = [
            "bytes_cloned", "bytes_copied", "bytes_hardlinked",
            "files_cloned", "files_copied", "files_hardlinked",
        ]
        res["io_stats"] = {k: int(m_io.group(i + 1)) for i, k in enumerate(keys)}
    m_sum = re.search(
        r"\[mitts::timings_sum\]\s+restore_sum_s=([\d\.]+)\s+restore_miss_sum_s=([\d\.]+)"
        r"\s+publish_sum_s=([\d\.]+)\s+rustc_sum_s=([\d\.]+)",
        text,
    )
    if m_sum:
        keys = ["restore_sum_s", "restore_miss_sum_s", "publish_sum_s", "rustc_sum_s"]
        res["timings_sum_s"] = {k: float(m_sum.group(i + 1)) for i, k in enumerate(keys)}
    m_bs = re.search(
        r"\[mitts::timings_sum\]\s+build_script_sum_s=([\d\.]+)\s+unit_key_sum_s=([\d\.]+)"
        r"\s+build_script_hits=(\d+)\s+build_script_misses=(\d+)",
        text,
    )
    if m_bs:
        res.setdefault("timings_sum_s", {})
        res["timings_sum_s"]["build_script_sum_s"] = float(m_bs.group(1))
        res["timings_sum_s"]["unit_key_sum_s"] = float(m_bs.group(2))
        res["build_script_hits"] = int(m_bs.group(3))
        res["build_script_misses"] = int(m_bs.group(4))
    m_ph = re.search(r"\[mitts::phases\]\s+compile_s=([\d\.]+)\s+test_exec_s=([\d\.]+)\s+total_s=([\d\.]+)", text)
    if m_ph:
        res["phases_s"] = {"compile_s": float(m_ph.group(1)), "test_exec_s": float(m_ph.group(2)), "total_s": float(m_ph.group(3))}
    return res


def sum_mitts_io(ios: list[dict]) -> dict:
    """Sums the `[mitts::io]` and `[mitts::timings_sum]` counters of several runs."""
    total: dict = {"runs_with_io": 0, "io_stats": {}, "timings_sum_s": {}, "build_script_hits": 0, "build_script_misses": 0}
    for io in ios:
        if "io_stats" in io:
            total["runs_with_io"] += 1
            for k, v in io["io_stats"].items():
                total["io_stats"][k] = total["io_stats"].get(k, 0) + v
        for k, v in io.get("timings_sum_s", {}).items():
            total["timings_sum_s"][k] = round(total["timings_sum_s"].get(k, 0.0) + v, 3)
        total["build_script_hits"] += io.get("build_script_hits", 0)
        total["build_script_misses"] += io.get("build_script_misses", 0)
    return total


def timed_resources(r: dict) -> dict:
    """CPU time and peak RSS of one timed run (rusage of the child process tree)."""
    return {"user_s": r.get("user_s"), "sys_s": r.get("sys_s"), "max_rss_mb": r.get("max_rss_mb")}


def resource_stats(runs: list[dict]) -> dict:
    """CPU time and peak RSS over the successful runs of one command."""
    ok = [r for r in runs if r.get("exit_code") == 0]
    return {
        "user_s": compute_stats([r["user_s"] for r in ok]),
        "sys_s": compute_stats([r["sys_s"] for r in ok]),
        "max_rss_mb": max((r["max_rss_mb"] for r in ok), default=None),
    }


def swarm_resources(runs: list[dict], arm: str) -> dict:
    """Per-worktree walls, summed CPU time, peak RSS and unit counters of one swarm run."""
    parsed = [parse_build_output(r["output"], arm) for r in runs]
    out = {
        "wt_wall_s": [r["wall_s"] for r in runs],
        "wt_user_s": [r["user_s"] for r in runs],
        "wt_sys_s": [r["sys_s"] for r in runs],
        "total_user_s": round(sum(r["user_s"] for r in runs), 3),
        "total_sys_s": round(sum(r["sys_s"] for r in runs), 3),
        "max_wt_rss_mb": max((r["max_rss_mb"] for r in runs), default=0.0),
        "sum_wt_rss_mb": round(sum(r["max_rss_mb"] for r in runs), 1),
        "units_compiled": sum(p["units_compiled"] for p in parsed),
        "units_cached": sum(p["units_cached"] for p in parsed),
        "units_total": sum(p["units_total"] for p in parsed),
        "wt_units_compiled": [p["units_compiled"] for p in parsed],
    }
    if arm == "mitts_cas":
        out["units_tainted"] = sum(p.get("units_tainted", 0) for p in parsed)
        out["fast_lock_hits"] = sum(1 for p in parsed if p.get("fast_lock") == 1)
        out["mitts_io"] = sum_mitts_io([parse_mitts_io(r["output"]) for r in runs])
    return out


def parse_sccache_stats(text: str) -> dict:
    def get_num(key: str) -> int:
        m = re.search(rf"{key}\s+(\d+)", text)
        return int(m.group(1)) if m else 0

    req = get_num("Compile requests")
    exec_req = get_num("Compile requests executed")
    hits = get_num("Cache hits")
    misses = get_num("Cache misses")
    hit_rate = round((hits / max(hits + misses, 1)) * 100.0, 2) if (hits + misses) > 0 else 0.0
    return {
        "compile_requests": req,
        "compile_requests_executed": exec_req,
        "cache_hits": hits,
        "cache_misses": misses,
        "hit_rate_pct": hit_rate,
    }


def compute_stats(vals: list[float]) -> dict:
    if not vals:
        return {"median": None, "q1": None, "q3": None, "iqr": None, "min": None, "max": None, "n": 0, "samples": []}
    s = sorted(vals)
    n = len(s)
    median = s[n // 2] if n % 2 == 1 else (s[n // 2 - 1] + s[n // 2]) / 2.0
    q1 = s[n // 4]
    q3 = s[(3 * n) // 4]
    return {
        "median": round(median, 3),
        "q1": round(q1, 3),
        "q3": round(q3, 3),
        "iqr": round(q3 - q1, 3) if n >= 2 else None,
        "min": round(s[0], 3),
        "max": round(s[-1], 3),
        "n": n,
        "samples": [round(v, 3) for v in vals],
    }


def spread_pct(vals: list[float]) -> float | None:
    if len(vals) < 2 or min(vals) <= 0:
        return None
    med = compute_stats(vals)["median"]
    return round(((max(vals) - min(vals)) / med) * 100.0, 2) if med else None

