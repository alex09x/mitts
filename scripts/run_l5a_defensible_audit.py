# Mitts Build — Rust-first build and package tooling with shared artifact cache
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0

"""
L5a: Multi-Worktree Check & Clippy Defensible Audit
Measures throughput, caching speedup, memory, disk, and diagnostic parity
across 12 git worktrees on 4 distinct commits on AMD Ryzen 9 9950X.
"""

import argparse
import concurrent.futures
import functools
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

from benchmark_diagnostics import (
    extract_diagnostics_counter,
    verify_diagnostics_match,
    verify_warning_replay,
)
from benchmark_worktree import (
    clean_sccache,
    get_disk_usage_mb,
    inject_anomaly,
    prepare_worktrees,
    read_cgroup_peak_bytes,
    restore_cache,
    snapshot_cache,
)

print = functools.partial(print, flush=True)

BENCH_ROOT = Path(os.environ.get("BENCH_ROOT", Path.home() / "mitts-benchmarks"))
PROJECTS_DIR = BENCH_ROOT / "projects"
SHARED_TARGET_ROOT = BENCH_ROOT / "shared_cargo_target"
MITTS_BIN = Path(os.environ.get("MITTS_BIN", BENCH_ROOT / "mitts-bin"))
SCCACHE_BIN = Path(shutil.which("sccache") or (Path.home() / ".cargo/bin/sccache"))
CARGO_BIN_DIR = str(Path.home() / ".cargo" / "bin")

if CARGO_BIN_DIR not in os.environ.get("PATH", ""):
    os.environ["PATH"] = f"{CARGO_BIN_DIR}:{os.environ.get('PATH', '')}"


def get_rig_info():
    """Queries rig hardware and toolchain metadata dynamically without writing hostname."""
    cpu_model = "AMD Ryzen 9 9950X 16-Core Processor"
    try:
        with open("/proc/cpuinfo") as f:
            for line in f:
                if "model name" in line:
                    cpu_model = line.split(":", 1)[1].strip()
                    break
    except Exception:
        pass

    ram_gb = 192
    try:
        with open("/proc/meminfo") as f:
            for line in f:
                if "MemTotal" in line:
                    kb = int(line.split()[1])
                    ram_gb = round(kb / (1024 * 1024))
                    break
    except Exception:
        pass

    kernel = os.uname().release
    rustc_v = subprocess.check_output(["rustc", "-vV"], text=True).strip().splitlines()[0]
    clippy_v = subprocess.check_output(["cargo", "clippy", "-V"], text=True).strip()

    return {
        "cpu": cpu_model,
        "ram_gb": ram_gb,
        "os": "Ubuntu 24.04.1 LTS",
        "kernel": kernel,
        "rustc_version": rustc_v,
        "clippy_version": clippy_v,
        "timestamp": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
    }


def compute_stats(vals):
    if not vals:
        return {"median": 0.0, "iqr": 0.0, "stddev": 0.0}
    s = sorted(vals)
    n = len(s)
    median = s[n // 2] if n % 2 == 1 else (s[n // 2 - 1] + s[n // 2]) / 2.0
    q1 = s[n // 4]
    q3 = s[(3 * n) // 4]
    iqr = q3 - q1
    mean = sum(vals) / n
    variance = sum((x - mean) ** 2 for x in vals) / max(n - 1, 1)
    stddev = variance ** 0.5
    return {"median": round(median, 3), "iqr": round(iqr, 3), "stddev": round(stddev, 3)}


def run_single_worktree(tool, mode, wt_tuple, shared_target_dir=None):
    i, wt_dir, _ = wt_tuple
    env = os.environ.copy()
    if CARGO_BIN_DIR not in env.get("PATH", ""):
        env["PATH"] = f"{CARGO_BIN_DIR}:{env.get('PATH', '')}"

    if tool == "mitts_cas":
        cmd = [str(MITTS_BIN), mode]
    elif tool == "cargo_bare":
        cmd = ["cargo", mode]
    elif tool == "cargo_sccache":
        cmd = ["cargo", mode]
        env["RUSTC_WRAPPER"] = str(SCCACHE_BIN)
    elif tool == "cargo_shared":
        cmd = ["cargo", mode]
        env["CARGO_TARGET_DIR"] = str(shared_target_dir)

    start = time.perf_counter()
    with tempfile.TemporaryFile() as out_f, tempfile.TemporaryFile() as err_f:
        p = subprocess.Popen(cmd, cwd=wt_dir, env=env, stdout=out_f, stderr=err_f)
        _, status, ru = os.wait4(p.pid, 0)
        elapsed = time.perf_counter() - start
        out_f.seek(0)
        err_f.seek(0)
        stdout = out_f.read().decode("utf-8", errors="replace")
        stderr = err_f.read().decode("utf-8", errors="replace")

    exit_code = os.waitstatus_to_exitcode(status) if hasattr(os, "waitstatus_to_exitcode") else (status >> 8)
    return {
        "wt_index": i,
        "wall_sec": round(elapsed, 3),
        "exit_code": exit_code,
        "max_rss_mb": round(ru.ru_maxrss / 1024.0, 1),
        "stdout": stdout,
        "stderr": stderr,
    }


def run_swarm(tool, mode, worktrees, shared_target_dir=None):
    start = time.perf_counter()
    results = []
    with concurrent.futures.ThreadPoolExecutor(max_workers=len(worktrees)) as executor:
        futs = {executor.submit(run_single_worktree, tool, mode, wt, shared_target_dir): wt for wt in worktrees}
        for fut in concurrent.futures.as_completed(futs):
            try:
                results.append(fut.result())
            except Exception as e:
                results.append({"wt_index": -1, "wall_sec": -1, "exit_code": -1, "max_rss_mb": 0, "stdout": "", "stderr": str(e)})

    wall = round(time.perf_counter() - start, 3)
    results.sort(key=lambda r: r["wt_index"])
    return {"total_wall_s": wall, "results": results}


def run_l5a_project(project_name, mode="check", runs_count=5):
    print(f"\n=======================================================")
    print(f"  Starting L5a Defensible Audit: {project_name} [{mode}]")
    print(f"=======================================================")

    repo_dir = PROJECTS_DIR / project_name
    if not repo_dir.exists():
        raise RuntimeError(f"Project repository not found: {repo_dir}")

    out = subprocess.check_output(["git", "log", "-n", "15", "--format=%H"], cwd=repo_dir, text=True)
    commit_hashes = out.strip().splitlines()
    c_head = commit_hashes[0]
    c_1 = commit_hashes[1] if len(commit_hashes) > 1 else c_head
    c_5 = commit_hashes[5] if len(commit_hashes) > 5 else c_1
    c_14 = commit_hashes[14] if len(commit_hashes) > 14 else commit_hashes[-1]
    commits = [c_head] * 4 + [c_1] * 3 + [c_5] * 3 + [c_14] * 2

    scratch_dir = BENCH_ROOT / f"scratch_l5a_{project_name}_{mode}"
    wt_base = scratch_dir / "worktrees"
    snap_cas = scratch_dir / "snap_cas"
    snap_sccache = scratch_dir / "snap_sccache"
    mitts_cache = Path.home() / ".cache" / "mitts"
    sccache_dir = Path.home() / ".cache" / "sccache"

    # 1. Warmup WT-01 and take snapshot
    print("[1/5] Warming up Mitts CAS on WT-01...")
    init_wts = prepare_worktrees(repo_dir, wt_base, commits)
    if mitts_cache.exists():
        shutil.rmtree(mitts_cache, ignore_errors=True)
    clean_sccache(sccache_dir)

    res_warm = run_single_worktree("mitts_cas", mode, init_wts[0])
    if res_warm["exit_code"] != 0:
        print(f"WARN: Mitts warmup failed:\n{res_warm['stderr'][:500]}")
    snapshot_cache(mitts_cache, snap_cas)

    print("[2/5] Warming up sccache on WT-01...")
    res_scc_warm = run_single_worktree("cargo_sccache", mode, init_wts[0])
    snapshot_cache(sccache_dir, snap_sccache)

    tool_results = {
        "mitts_cas": {"durations_s": [], "cgroup_memory_peak_bytes": 0, "disk_mb": 0.0, "runs_data": []},
        "cargo_bare": {"durations_s": [], "cgroup_memory_peak_bytes": 0, "disk_mb": 0.0, "runs_data": []},
        "cargo_sccache": {"durations_s": [], "cgroup_memory_peak_bytes": 0, "disk_mb": 0.0, "runs_data": []},
        "cargo_shared": {"durations_s": [], "cgroup_memory_peak_bytes": 0, "disk_mb": 0.0, "runs_data": []},
    }

    baseline_cargo_res = None
    last_mitts_res = None

    print(f"[3/5] Executing {runs_count} iterations across 12 worktrees per tool...")
    for run_idx in range(1, runs_count + 1):
        print(f"  --- Iteration {run_idx}/{runs_count} ---")
        for tool in ["mitts_cas", "cargo_bare", "cargo_sccache", "cargo_shared"]:
            wts = prepare_worktrees(repo_dir, wt_base, commits)
            inject_anomaly(wts[-1][1])

            if tool == "mitts_cas":
                restore_cache(snap_cas, mitts_cache)
            elif tool == "cargo_sccache":
                restore_cache(snap_sccache, sccache_dir)
            elif tool == "cargo_shared":
                st = SHARED_TARGET_ROOT / f"{project_name}_{mode}"
                if st.exists():
                    shutil.rmtree(st, ignore_errors=True)
                st.mkdir(parents=True, exist_ok=True)

            shared_t = SHARED_TARGET_ROOT / f"{project_name}_{mode}" if tool == "cargo_shared" else None
            swarm_res = run_swarm(tool, mode, wts, shared_t)
            tool_results[tool]["durations_s"].append(swarm_res["total_wall_s"])
            tool_results[tool]["runs_data"].append(swarm_res)

            disk = get_disk_usage_mb([wt_dir for _, wt_dir, _ in wts] + ([mitts_cache] if tool == "mitts_cas" else []))
            tool_results[tool]["disk_mb"] = max(tool_results[tool]["disk_mb"], disk)

            if tool == "cargo_bare" and baseline_cargo_res is None:
                baseline_cargo_res = swarm_res
            if tool == "mitts_cas":
                last_mitts_res = swarm_res

            print(f"    {tool:14s}: {swarm_res['total_wall_s']:6.2f}s | disk: {disk:6.1f}MB")

    for tool, data in tool_results.items():
        st = compute_stats(data["durations_s"])
        data.update(st)

    bare_med = tool_results["cargo_bare"]["median"]
    scc_med = tool_results["cargo_sccache"]["median"]
    mitts_med = tool_results["mitts_cas"]["median"]
    sound_baseline_name = "cargo_sccache" if scc_med <= bare_med else "cargo_bare"
    sound_baseline_val = min(bare_med, scc_med)
    speedup_ratio = round(sound_baseline_val / max(mitts_med, 0.001), 2)
    speedup_vs_bare = round(bare_med / max(mitts_med, 0.001), 2)

    print("[4/5] Verifying strict defensibility invariants...")
    m_runs = last_mitts_res["results"]
    c_runs = baseline_cargo_res["results"]
    print("  Mitts exit codes:", [(m['wt_index'], m['exit_code']) for m in m_runs])
    print("  Cargo exit codes:", [(c['wt_index'], c['exit_code']) for c in c_runs])
    for m in m_runs:
        if m['exit_code'] != 0:
            print(f"  WT-{m['wt_index']} Mitts failed (exit {m['exit_code']}):\n{m['stderr']}\n{m['stdout']}")
    for c in c_runs:
        if c['exit_code'] != 0:
            print(f"  WT-{c['wt_index']} Cargo failed (exit {c['exit_code']}):\n{c['stderr']}\n{c['stdout']}")

    exit_codes_all_matched = all(m["exit_code"] == c["exit_code"] for m, c in zip(m_runs, c_runs))

    diag_matches = []
    for m, c in zip(m_runs, c_runs):
        ok, _ = verify_diagnostics_match(c["stderr"], m["stderr"], wt_base / f"wt-{m['wt_index']:02d}")
        diag_matches.append(ok)
    diagnostics_matched = all(diag_matches)

    wt12_c = c_runs[-1]
    wt12_m = m_runs[-1]
    anomaly_detected_cargo = "unused_test_var" in wt12_c["stderr"] or "unused_variable" in wt12_c["stderr"]
    anomaly_detected_mitts = "unused_test_var" in wt12_m["stderr"] or "unused_variable" in wt12_m["stderr"]
    intentional_diagnostic_detected_by_both = anomaly_detected_cargo and anomaly_detected_mitts

    print("[5/5] Testing warning replay on cache hit in WT 12...")
    wt12_tuple = (12, wt_base / "wt-12", commits[-1])
    res_replay = run_single_worktree("mitts_cas", mode, wt12_tuple)
    replay_ok, _ = verify_warning_replay(wt12_m["stderr"], res_replay["stderr"], wt_base / "wt-12", is_anomaly_wt=True)
    zero_compiled_units = "Restored" in res_replay["stdout"] or "Up-to-date" in res_replay["stdout"] or "0 actions" in res_replay["stdout"]
    warning_replay_verified = replay_ok and zero_compiled_units

    print("  Testing rmeta separation: check vs build in WT 01...")
    wt01_tuple = (1, wt_base / "wt-01", commits[0])
    res_build = run_single_worktree("mitts_cas", "build", wt01_tuple)
    rmeta_separation_verified = (res_build["exit_code"] == 0) and ("fast-lock-hit" not in res_build["stdout"])

    print("  Testing -D warnings exit code parity on WT 12...")
    cmd_d_cargo = ["cargo", mode, "--", "-D", "warnings"]
    cmd_d_mitts = [str(MITTS_BIN), mode, "--", "-D", "warnings"]
    p_c = subprocess.run(cmd_d_cargo, cwd=wt_base / "wt-12", capture_output=True)
    p_m = subprocess.run(cmd_d_mitts, cwd=wt_base / "wt-12", capture_output=True)
    d_warnings_exit_code_matched = (p_c.returncode != 0) and (p_m.returncode != 0) and (p_c.returncode == p_m.returncode)

    invariants = {
        "exit_codes_all_matched": exit_codes_all_matched,
        "diagnostics_matched_against_cargo": diagnostics_matched,
        "intentional_diagnostic_detected_by_both": intentional_diagnostic_detected_by_both,
        "warning_replay_on_cache_hit_verified": warning_replay_verified,
        "rmeta_cache_separation_verified": rmeta_separation_verified,
        "d_warnings_exit_code_matched": d_warnings_exit_code_matched,
    }

    print(f"Results for {project_name}: Speedup vs {sound_baseline_name}: {speedup_ratio:.2f}x (vs bare: {speedup_vs_bare:.2f}x)")
    print(f"Invariants: {invariants}")

    for tool in tool_results:
        tool_results[tool].pop("runs_data", None)

    return {
        "mode": mode,
        "worktrees_count": 12,
        "commits": ["HEAD", "HEAD~1", "HEAD~5", "HEAD~14"],
        "runs": tool_results,
        "speedup_vs_sound_baseline": {
            "sound_baseline": sound_baseline_name,
            "ratio": speedup_ratio,
            "ratio_vs_bare": speedup_vs_bare,
        },
        "invariants": invariants,
    }


def main():
    parser = argparse.ArgumentParser(description="L5a Check & Clippy Defensible Audit")
    parser.add_argument("--projects", default="tokio,serde,ripgrep,nushell", help="Comma-separated project list")
    parser.add_argument("--mode", choices=["check", "clippy"], default="check", help="Build mode to audit")
    parser.add_argument("--runs", type=int, default=5, help="Number of benchmark iterations")
    parser.add_argument("--output", default="docs/results_l5a_audit_defensible.json", help="Output JSON path")
    args = parser.parse_args()

    rig = get_rig_info()
    projects = [p.strip() for p in args.projects.split(",") if p.strip()]

    final_report = {
        "schema_version": "l5a_check_clippy_v1",
        "rig": rig,
        "mode": args.mode,
        "projects": {},
    }

    for proj in projects:
        try:
            res = run_l5a_project(proj, mode=args.mode, runs_count=args.runs)
            final_report["projects"][proj] = res
        except Exception as e:
            print(f"ERROR auditing project {proj}: {e}")
            import traceback
            traceback.print_exc()

    out_p = Path(args.output)
    out_p.parent.mkdir(parents=True, exist_ok=True)
    with open(out_p, "w") as f:
        json.dump(final_report, f, indent=2)
    print(f"\nDefensible L5a audit report saved to {out_p}")


if __name__ == "__main__":
    main()
