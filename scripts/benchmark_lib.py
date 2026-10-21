#!/usr/bin/env python3
# Mitts Build — Rust-first build and package tooling with shared artifact cache
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0

"""
Environment setup, cache relocation (`--cache-dir`), sccache daemon lifecycle,
command builders for all build arms, and invariant evaluators for `benchmark_project.py`.
"""

from __future__ import annotations

import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

from benchmark_common import CARGO_BIN_DIR, get_binary_provenance, get_filesystem_type, get_rig_info
from benchmark_snapshot import purge_mitts_cas, restore_dir, snapshot_dir
from benchmark_diagnostics import (
    extract_diagnostics_counter, verify_anomaly_present, verify_diagnostics_match, verify_warning_replay,
)
from benchmark_worktree import (
    DEFECT_TEST_NAME, assert_dir_empty_or_absent, chmod_rmtree, copy_repo_tree, ensure_commit_history,
    git_checkout_clean, prepare_commit_locks_and_fetch, prepare_swarm_worktrees, restore_commit_lock,
)
from t5_parsers import compute_stats, parse_sccache_stats, spread_pct

TOOLS = ["mitts_cas", "cargo_bare", "cargo_sccache", "cargo_shared"]
SOUND_TOOLS = ["mitts_cas", "cargo_bare", "cargo_sccache"]
DEFAULT_SCCACHE_SIZE = "50G"

__all__ = [
    "TOOLS", "SOUND_TOOLS", "DEFAULT_SCCACHE_SIZE", "assert_dir_empty_or_absent", "chmod_rmtree",
    "collect_provenance", "collect_sccache_stats_and_stop", "copy_repo_tree", "ensure_commit_history",
    "evaluate_t4_invariants", "evaluate_t5_invariants", "git_checkout_clean", "parse_explain_metrics",
    "prepare_commit_locks_and_fetch", "prepare_swarm_worktrees", "purge_mitts_cas", "reset_sccache",
    "restore_commit_lock", "restore_dir", "run_timed", "setup_environments", "snapshot_dir",
    "start_sccache_zero_stats", "stop_sccache", "tool_cmd", "tool_disk_paths", "verify_diagnostics_parity",
]


def stop_sccache(sccache_bin: Path, sccache_dir: Path | None = None) -> None:
    env = os.environ.copy()
    if sccache_dir:
        env["SCCACHE_DIR"] = str(sccache_dir)
    subprocess.run([str(sccache_bin), "--stop-server"], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def start_sccache_zero_stats(sccache_bin: Path, env: dict[str, str]) -> None:
    stop_sccache(sccache_bin)
    subprocess.run([str(sccache_bin), "--start-server"], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    subprocess.run([str(sccache_bin), "--zero-stats"], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def reset_sccache(sccache_bin: Path, sccache_dir: Path, env: dict[str, str]) -> None:
    stop_sccache(sccache_bin, sccache_dir)
    chmod_rmtree(sccache_dir)
    assert_dir_empty_or_absent(sccache_dir)
    sccache_dir.mkdir(parents=True, exist_ok=True)
    start_sccache_zero_stats(sccache_bin, env)


def collect_sccache_stats_and_stop(sccache_bin: Path, env: dict[str, str]) -> dict:
    stats_out = ""
    try:
        stats_out = subprocess.check_output([str(sccache_bin), "--show-stats"], env=env, text=True, stderr=subprocess.DEVNULL)
    except Exception:
        pass
    stop_sccache(sccache_bin)
    return parse_sccache_stats(stats_out)


def setup_environments(
    work_dir: Path,
    cache_dir: Path,
    mitts_bin: Path,
    sccache_bin: Path,
    shared_cargo_home: Path | None = None,
) -> dict:
    mitts_cas = cache_dir / "mitts_cas"
    sccache_dir = cache_dir / "sccache"
    shared_target = cache_dir / "cargo_shared_target"
    cargo_home = shared_cargo_home or (cache_dir / "cargo_home")
    for d in (mitts_cas, sccache_dir, shared_target, cargo_home):
        d.mkdir(parents=True, exist_ok=True)

    # Never copy ~/.cargo/config.toml into cargo_home: host configs may set rustc-wrapper=sccache.
    for cfg_name in ("config.toml", "config"):
        if (cargo_home / cfg_name).exists():
            chmod_rmtree(cargo_home / cfg_name)

    real_cargo = Path(os.environ.get("CARGO_HOME", Path.home() / ".cargo"))
    bin_dst = cargo_home / "bin"
    if (real_cargo / "bin").exists() and not bin_dst.exists():
        os.symlink(real_cargo / "bin", bin_dst)

    for sub in ("registry/index", "registry/cache", "git/db"):
        src_sub = real_cargo / sub
        dst_sub = cargo_home / sub
        if src_sub.exists() and not dst_sub.exists():
            dst_sub.parent.mkdir(parents=True, exist_ok=True)
            shutil.copytree(src_sub, dst_sub, symlinks=True)
    (cargo_home / "registry" / "src").mkdir(parents=True, exist_ok=True)
    (cargo_home / "git" / "checkouts").mkdir(parents=True, exist_ok=True)

    base_env = os.environ.copy()
    if CARGO_BIN_DIR not in base_env.get("PATH", ""):
        base_env["PATH"] = f"{CARGO_BIN_DIR}:{base_env.get('PATH', '')}"
    base_env["CARGO_HOME"] = str(cargo_home)
    base_env["CARGO_TERM_COLOR"] = "never"
    base_env["CI"] = "true"
    for k in (
        "RUSTC_WRAPPER", "CARGO_BUILD_RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER",
        "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER", "CARGO_TARGET_DIR", "SCCACHE_DIR",
        "MITTS_CACHE_DIR", "CARGO_INCREMENTAL", "MITTS_NO_REFLINK", "MITTS_TIMINGS",
    ):
        base_env.pop(k, None)
    # Explicitly empty RUSTC_WRAPPER overrides any ancestor ~/.cargo/config.toml rustc-wrapper
    base_env["RUSTC_WRAPPER"] = ""
    base_env["CARGO_BUILD_RUSTC_WRAPPER"] = ""

    fetch_env = base_env.copy()
    offline_env = {**base_env, "CARGO_NET_OFFLINE": "true"}

    env_mitts = {**offline_env, "MITTS_CACHE_DIR": str(mitts_cas)}
    env_bare = offline_env.copy()
    env_bare_no_incr = {**offline_env, "CARGO_INCREMENTAL": "0"}
    env_sccache = {
        **offline_env,
        "RUSTC_WRAPPER": str(sccache_bin),
        "CARGO_BUILD_RUSTC_WRAPPER": str(sccache_bin),
        "SCCACHE_DIR": str(sccache_dir),
        "SCCACHE_CACHE_SIZE": DEFAULT_SCCACHE_SIZE,
        "CARGO_INCREMENTAL": "0",
    }
    env_shared = {**offline_env, "CARGO_TARGET_DIR": str(shared_target)}

    return {
        "mitts_bin": mitts_bin,
        "sccache_bin": sccache_bin,
        "mitts_cas": mitts_cas,
        "sccache_dir": sccache_dir,
        "shared_target": shared_target,
        "cargo_home": cargo_home,
        "cargo_locked": True,
        "fetch_env": fetch_env,
        "envs": {
            "mitts_cas": env_mitts,
            "cargo_bare": env_bare,
            "cargo_bare_no_incr": env_bare_no_incr,
            "cargo_sccache": env_sccache,
            "cargo_shared": env_shared,
        },
    }


def tool_cmd(tool: str, subcmd: str, ctx: dict, extra_args: list[str] | None = None) -> list[str]:
    extra = extra_args or []
    m_bin = str(ctx["mitts_bin"])
    m_cas = str(ctx["mitts_cas"])
    locked = ["--locked"] if ctx.get("cargo_locked", True) else []
    if tool == "mitts_cas":
        if subcmd == "build":
            return [m_bin, "build", "--cache-dir", m_cas] + (extra + ["--", "--workspace"] if "--exclude" in extra else extra)
        if subcmd == "check":
            return [m_bin, "check", "--workspace", "--cache-dir", m_cas] + extra
        if subcmd == "clippy":
            return [m_bin, "clippy", "--workspace", "--cache-dir", m_cas] + extra
        if subcmd == "test_no_run":
            return [m_bin, "test", "--workspace", "--tests", "--no-run", "--cache-dir", m_cas] + extra
        if subcmd == "test_smoke":
            return [m_bin, "test", "--workspace", "--tests", "--no-fail-fast", "--cache-dir", m_cas, DEFECT_TEST_NAME] + extra
    else:
        if subcmd == "build":
            return ["cargo", "build", "--offline"] + locked + (["--workspace"] + extra if "--exclude" in extra else extra)
        if subcmd == "check":
            return ["cargo", "check", "--workspace", "--offline"] + locked + extra
        if subcmd == "clippy":
            return ["cargo", "clippy", "--workspace", "--offline"] + locked + extra
        if subcmd == "test_no_run":
            return ["cargo", "test", "--workspace", "--tests", "--no-run", "--offline"] + locked + extra
        if subcmd == "test_smoke":
            return ["cargo", "test", "--workspace", "--tests", "--no-fail-fast", "--offline"] + locked + extra + ["--", DEFECT_TEST_NAME]
    raise ValueError(f"Unknown subcmd: {subcmd}")


def tool_disk_paths(tool: str, wt_dirs: list[Path], ctx: dict) -> list[Path]:
    t_dirs = [w / "target" for w in wt_dirs]
    if tool == "mitts_cas":
        cas_subdirs = [p for p in ctx["mitts_cas"].iterdir() if p.name != "sources"] if ctx["mitts_cas"].exists() else []
        return t_dirs + cas_subdirs
    if tool == "cargo_sccache":
        return t_dirs + [ctx["sccache_dir"]]
    if tool == "cargo_shared":
        return [ctx["shared_target"]]
    return t_dirs


def run_timed(cmd: list[str], cwd: Path, env: dict[str, str]) -> dict:
    t0 = time.perf_counter()
    with tempfile.TemporaryFile(mode="w+", encoding="utf-8", errors="replace") as tf:
        p = subprocess.Popen(cmd, cwd=cwd, env=env, stdout=tf, stderr=subprocess.STDOUT, text=True)
        _, status, ru = os.wait4(p.pid, 0)
        code = os.waitstatus_to_exitcode(status) if hasattr(os, "waitstatus_to_exitcode") else (status >> 8)
        p.returncode = code
        wall_s = time.perf_counter() - t0
        tf.seek(0)
        out = tf.read()
    rss_div = (1024.0 * 1024.0) if sys.platform == "darwin" else 1024.0
    lines = out.strip().splitlines() if out else []
    tail_str = ""
    if code != 0 and lines:
        tail40 = lines[-40:]
        err_re = re.compile(r"(^\s*error(\[E\d+\])?:|sccache:\s*error|caused by:|os error \d+|too many open files)", re.I)
        first_err = next((ln for ln in lines if err_re.search(ln)), None)
        tail_str = (f"[first_error] {first_err}\n" if (first_err and first_err not in tail40) else "") + "\n".join(tail40)
    return {
        "wall_s": round(wall_s, 4), "user_s": round(ru.ru_utime, 3), "sys_s": round(ru.ru_stime, 3),
        "max_rss_mb": round(ru.ru_maxrss / rss_div, 1), "exit_code": code, "output": out, "stderr_tail": tail_str,
    }


def parse_explain_metrics(output: str) -> dict:
    info: dict = {"units": 0, "timings_ms": {}}
    for line in output.splitlines():
        if line.startswith("[mitts::timings]"):
            for m in re.finditer(r"(\w+)=([\d.]+)ms", line):
                info["timings_ms"][m.group(1)] = float(m.group(2))
    try:
        s, e = output.find("{"), output.rfind("}")
        if s != -1 and e > s:
            data = json.loads(output[s : e + 1])
            info["units"] = len(data.get("units", data.get("compilation_units", [])))
    except Exception:
        pass
    return info


def verify_diagnostics_parity(cargo_out: str, mitts_out: str, cached_out: str, ws_root: Path, anomaly_info: dict | None = None) -> dict:
    matched, match_msg = verify_diagnostics_match(cargo_out, mitts_out, ws_root)
    replayed, replay_msg = verify_warning_replay(mitts_out, cached_out, ws_root, is_anomaly_wt=True)
    return {
        "cargo_diagnostics": sum(extract_diagnostics_counter(cargo_out, ws_root).values()),
        "mitts_diagnostics": sum(extract_diagnostics_counter(mitts_out, ws_root).values()),
        "anomaly_injected": anomaly_info,
        "anomaly_present_in_cargo": verify_anomaly_present(cargo_out, ws_root, anomaly_info),
        "anomaly_present_in_mitts": verify_anomaly_present(mitts_out, ws_root, anomaly_info),
        "anomaly_present_in_cached_replay": verify_anomaly_present(cached_out, ws_root, anomaly_info),
        "exact_match_cargo_vs_mitts": matched,
        "match_detail": match_msg,
        "exact_replay_on_cache_hit": replayed,
        "replay_detail": replay_msg,
    }


def evaluate_t4_invariants(l5_res: dict) -> dict:
    inv: dict = {}
    for m_key, m_name in (("l5a", "check"), ("l5b", "clippy")):
        md = l5_res[m_name]
        dp = md["diagnostics_parity"]
        inv[f"{m_key}_all_sound_builds_exit_zero"] = all(md[t]["failures"] == 0 for t in SOUND_TOOLS)
        inv[f"{m_key}_diagnostics_parity_ok"] = bool(
            dp["exact_match_cargo_vs_mitts"] and dp["exact_replay_on_cache_hit"] and dp["anomaly_present_in_mitts"]
        )
    inv["all_t4_sound_and_parity_ok"] = all(inv.values())
    return inv


def evaluate_t5_invariants(
    arm_series: dict, fast_lock_noop: dict, unsound_testable: bool = True, swarm_lib_sources_distinct: bool = False
) -> dict:
    runs_n = len(arm_series["cargo_bare"])
    builds_ok = all(r["build_failures"] == 0 for t in SOUND_TOOLS for r in arm_series[t])
    counts_match = True
    for i in range(runs_n):
        for w_idx in range(len(arm_series["cargo_bare"][i]["wt_binary_counts"])):
            c_set = {arm_series[t][i]["wt_binary_counts"][w_idx] for t in SOUND_TOOLS}
            if len(c_set) != 1 or 0 in c_set:
                counts_match = False
    defect_caught = all(r["smoke_wt12_defect_caught"] for t in SOUND_TOOLS for r in arm_series[t])
    clean_ok = all(r["smoke_wt01_clean_ok"] for t in SOUND_TOOLS for r in arm_series[t])
    c_wt1 = [r["wt01_wall_s"] for r in arm_series["cargo_bare"] if r["build_failures"] == 0]
    c_spread = spread_pct(c_wt1)
    total_spread_by_arm: dict[str, float | None] = {}
    total_iqr_by_arm: dict[str, float | None] = {}
    for t in TOOLS:
        tw = [r["total_wall_s"] for r in arm_series[t] if r["build_failures"] == 0]
        total_spread_by_arm[t] = spread_pct(tw)
        st = compute_stats(tw)
        total_iqr_by_arm[t] = round((st["iqr"] / st["median"]) * 100.0, 2) if (st["iqr"] is not None and st["median"]) else None
    base_bins = arm_series["cargo_bare"][0]["wt_binary_counts"][0] if runs_n and arm_series["cargo_bare"][0]["wt_binary_counts"] else 0
    sh_runs = arm_series["cargo_shared"]
    return {
        "all_sound_builds_exit_zero": builds_ok,
        "test_binaries_count_matches_all_sound_arms": counts_match,
        "defect_wt12_caught_all_sound_arms": defect_caught,
        "clean_wt01_smoke_ok_all_sound_arms": clean_ok,
        "all_t5_sound_and_parity_ok": bool(builds_ok and counts_match and defect_caught and clean_ok),
        "total_wall_spread_pct_by_arm": total_spread_by_arm,
        "total_wall_iqr_pct_by_arm": total_iqr_by_arm,
        "sound_arms_total_wall_variance_le_15_pct": all(
            total_spread_by_arm[t] is not None and total_spread_by_arm[t] <= 15.0 for t in SOUND_TOOLS
        ),
        "all_arms_total_wall_variance_le_15_pct": all(
            v is not None and v <= 15.0 for v in total_spread_by_arm.values()
        ),
        "cargo_bare_wt1_variance_pct": c_spread,
        "cargo_bare_wt1_variance_le_15_pct": c_spread is not None and c_spread <= 15.0,
        "variance_gate_note": (
            "NOISY gate checks (max-min)/median <= 15% on total_wall_s across all sound arms (mitts_cas, cargo_bare, cargo_sccache); "
            "all_arms_total_wall_variance_le_15_pct (including unsound cargo_shared) and cargo_bare_wt1_variance_pct are retained "
            "as informational fields."
        ),
        "fast_lock_noop_same_binary_count": fast_lock_noop["exit_code"] == 0 and fast_lock_noop["test_binaries_count"] == base_bins and base_bins > 0,
        "swarm_lib_sources_distinct": swarm_lib_sources_distinct,
        "unsound_testable": unsound_testable,
        "cargo_shared_unsound_observed": any(not r["smoke_wt12_defect_caught"] or not r["smoke_wt01_clean_ok"] or r["build_failures"] > 0 for r in sh_runs),
    }


def collect_provenance(
    mitts_bin: Path,
    sccache_bin: Path,
    work_dir: Path,
    cache_dir: Path,
    script_files: list[Path],
    max_load: float,
    background_note: str,
    max_busy: float | None = None,
    baseline_busy: float | None = None,
    effective_busy: float | None = None,
) -> dict:
    prov = get_binary_provenance(mitts_bin, sccache_bin)
    prov["sccache_cache_size"] = DEFAULT_SCCACHE_SIZE
    prov["effective_rustc_wrapper"] = {
        "mitts_cas": "",
        "cargo_bare": "",
        "cargo_bare_no_incr": "",
        "cargo_sccache": str(sccache_bin),
        "cargo_shared": "",
    }
    prov["ci_env_rationale"] = "CI=true disables interactive progress bars and prompts across both cargo and mitts."
    script_files = [*script_files, Path(__file__).with_name("benchmark_snapshot.py")]
    prov["scripts_provenance"] = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in script_files if p.exists()}
    prov["rig"] = get_rig_info()
    prov["preflight_policy"] = {
        "max_busy_cpu_fraction": max_busy,
        "cpu_busy_baseline": baseline_busy,
        "effective_max_busy": effective_busy,
        "max_load": max_load,
        "gate": "cpu busy fraction over a 1 s sample must not exceed max(max_busy, baseline + 0.10) and no foreign compiler process may run; the baseline is the median of 1 s samples taken before the run; foreign load that holds within 0.10 over 5 consecutive samples with no compiler process is adopted as the baseline (released once the rig is back under the original threshold); a gate timeout proceeds and flags the project GATE_TIMEOUT; every adoption, release and timeout is listed in the project's gate_events; loadavg is recorded and gates only without a CPU sample",
        "background_note": background_note,
    }
    prov["storage"] = {
        "worktree_filesystem": get_filesystem_type(work_dir),
        "cache_filesystem": get_filesystem_type(cache_dir),
        "same_device": os.stat(work_dir).st_dev == os.stat(cache_dir).st_dev,
    }
    return prov
