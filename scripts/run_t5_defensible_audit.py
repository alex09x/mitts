# Mitts Build — Rust-first build and package tooling with shared artifact cache
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0

"""
T5: multi-worktree build swarm, defensible audit.

Twelve git worktrees of one project on four commits build their whole test
suite in parallel (`test --no-run`) with Mitts, plain cargo, cargo + sccache
and cargo with a shared target directory. The metric is the wall-clock time of
the build swarm: Mitts is a build tool, and the time a test suite takes to run
is the same for every arm, so test execution is kept out of the measurement.

Soundness is checked after each swarm with two short executions that are timed
separately and excluded from the metric: worktree 12 carries an injected
failing test that every sound arm must report as failed, and worktree 1 runs
every built binary with the same exact filter and must report them all clean.
"""

import argparse
import concurrent.futures
import functools
import hashlib
import json
import os
import shutil
import subprocess
import tempfile
import time
from pathlib import Path

from benchmark_common import (
    check_quiet_rig,
    get_binary_provenance,
    get_filesystem_type,
    get_rig_info,
    wait_for_quiet_rig,
)
from benchmark_pressure import (
    SystemPressureSampler,
    get_current_dirty_mb,
    get_device_sectors_written,
    sync_and_wait_dirty,
)
from benchmark_worktree import (
    clean_sccache,
    get_disk_usage_mb,
    prepare_worktrees,
    restore_cache,
    snapshot_cache,
)
from t5_parsers import (
    DEFECT_TEST_NAME,
    compute_stats,
    extract_failed_tests,
    inject_test_defect,
    parse_build_output,
    parse_mitts_io,
    parse_mitts_stats,
    parse_sccache_stats,
    parse_test_results,
    spread_pct,
)

print = functools.partial(print, flush=True)

SCHEMA_VERSION = "t5_build_swarm_v2"
SCRIPT_FILES = [
    "run_t5_defensible_audit.py",
    "benchmark_worktree.py",
    "benchmark_common.py",
    "benchmark_pressure.py",
    "t5_parsers.py",
]
TOOLS = ["mitts_cas", "cargo_bare", "cargo_sccache", "cargo_shared"]
SOUND_TOOLS = ["mitts_cas", "cargo_bare", "cargo_sccache"]
PREFLIGHT_TIMEOUT_S = 300.0
PREFLIGHT_MAX_LOAD = 1.0

BENCH_ROOT = Path(os.environ.get("BENCH_ROOT", Path.home() / "mitts-benchmarks"))
DEFAULT_RESULTS_DIR = Path(os.environ.get("BENCHMARK_OUTPUT_DIR", BENCH_ROOT / "benchmark-results"))
PROJECTS_DIR = BENCH_ROOT / "projects"
SHARED_TARGET_ROOT = BENCH_ROOT / "shared_cargo_target"
MITTS_BIN = Path(os.environ.get("MITTS_BIN", BENCH_ROOT / "mitts-bin"))
SCCACHE_BIN = Path(shutil.which("sccache") or (Path.home() / ".cargo/bin/sccache"))
CARGO_BIN_DIR = str(Path.home() / ".cargo" / "bin")

if CARGO_BIN_DIR not in os.environ.get("PATH", ""):
    os.environ["PATH"] = f"{CARGO_BIN_DIR}:{os.environ.get('PATH', '')}"


# --------------------------------------------------------------------------- provenance

def check_same_device(path_a, path_b):
    if not path_a or not path_b:
        return None
    try:
        pa = Path(path_a).resolve()
        while not pa.exists() and pa.parent != pa:
            pa = pa.parent
        pb = Path(path_b).resolve()
        while not pb.exists() and pb.parent != pb:
            pb = pb.parent
        return os.stat(pa).st_dev == os.stat(pb).st_dev
    except Exception:
        return None


def get_scripts_provenance():
    scripts_dir = Path(__file__).resolve().parent
    script_shas = {}
    for name in SCRIPT_FILES:
        p = scripts_dir / name
        if p.exists():
            script_shas[name] = hashlib.sha256(p.read_bytes()).hexdigest()
    combined = hashlib.sha256(
        "".join(f"{k}:{v}" for k, v in sorted(script_shas.items())).encode("utf-8")
    ).hexdigest()

    git_head = None
    scripts_dirty = None
    try:
        git_dir = Path(os.environ.get("MITTS_REPO", ""))
        if not git_dir.exists():
            git_dir = (
                scripts_dir
                if (scripts_dir / ".git").exists() or (scripts_dir.parent / ".git").exists()
                else (Path.home() / "mitts")
            )
        git_head = subprocess.check_output(
            ["git", "-C", str(git_dir), "rev-parse", "HEAD"], text=True, stderr=subprocess.DEVNULL
        ).strip()
        scripts_dirty = False
        for sname, s_sha in script_shas.items():
            blob = subprocess.check_output(
                ["git", "-C", str(git_dir), "show", f"HEAD:scripts/{sname}"], stderr=subprocess.DEVNULL
            )
            if hashlib.sha256(blob).hexdigest() != s_sha:
                scripts_dirty = True
                break
    except Exception:
        git_head = git_head if scripts_dirty is not None else None
        scripts_dirty = None if git_head is None else True

    return {
        "combined_sha256": combined,
        "scripts": script_shas,
        "git_commit": git_head,
        "scripts_dirty": scripts_dirty,
    }


def get_provenance_info(mitts_cache=None, worktree_dir=None):
    prov = get_binary_provenance(MITTS_BIN)
    prov["scripts_provenance"] = get_scripts_provenance()
    prov["diagnostic_flags"] = {
        "mitts_no_reflink": os.environ.get("MITTS_NO_REFLINK", "0"),
        "mitts_no_hardlink": os.environ.get("MITTS_NO_HARDLINK", "0"),
        "mitts_timings": os.environ.get("MITTS_TIMINGS", "0"),
    }
    if mitts_cache:
        prov["cache_filesystem"] = get_filesystem_type(Path(mitts_cache))
    if worktree_dir:
        prov["worktree_filesystem"] = get_filesystem_type(Path(worktree_dir))
    if mitts_cache and worktree_dir:
        prov["same_device"] = check_same_device(mitts_cache, worktree_dir)
    is_quiet, load_1m, foreign_procs = check_quiet_rig()
    prov["preflight"] = {"loadavg_1m": load_1m, "foreign_processes": foreign_procs, "is_quiet": is_quiet}
    return prov


# --------------------------------------------------------------------------- one worktree

def _tool_env(tool, wt_dir, shared_target_dir):
    env = os.environ.copy()
    if CARGO_BIN_DIR not in env.get("PATH", ""):
        env["PATH"] = f"{CARGO_BIN_DIR}:{env.get('PATH', '')}"
    env["CARGO"] = str(shutil.which("cargo") or (Path.home() / ".cargo/bin/cargo"))
    env["RUSTUP_HOME"] = str(Path.home() / ".rustup")
    env["CARGO_HOME"] = str(Path.home() / ".cargo")
    env["MITTS_TIMINGS"] = "1"
    wt_tmp = (wt_dir / "tmp").resolve()
    wt_tmp.mkdir(parents=True, exist_ok=True)
    env["TMPDIR"] = str(wt_tmp)
    if tool == "cargo_sccache":
        env["RUSTC_WRAPPER"] = str(SCCACHE_BIN)
    elif tool == "cargo_shared":
        env["CARGO_TARGET_DIR"] = str(shared_target_dir)
    return env


def _test_cmd(tool, is_workspace, extra_build_args, no_run, filter_exact=None):
    ws = ["--workspace"] if is_workspace else []
    base = [str(MITTS_BIN), "test", "--tests"] if tool == "mitts_cas" else ["cargo", "test", "--offline", "--tests"]
    cmd = base + ws + ["--no-fail-fast"] + list(extra_build_args or [])
    if no_run:
        return cmd + ["--no-run"]
    if filter_exact:
        if tool == "mitts_cas":
            cmd += [filter_exact, "--exact"]
        else:
            cmd += ["--", "--exact", filter_exact]
    return cmd


def _run(cmd, cwd, env):
    start = time.perf_counter()
    with tempfile.TemporaryFile() as out_f, tempfile.TemporaryFile() as err_f:
        p = subprocess.Popen(cmd, cwd=cwd, env=env, stdout=out_f, stderr=err_f)
        _, status, ru = os.wait4(p.pid, 0)
        elapsed = time.perf_counter() - start
        out_f.seek(0)
        err_f.seek(0)
        stdout = out_f.read().decode("utf-8", errors="replace")
        stderr = err_f.read().decode("utf-8", errors="replace")
    exit_code = os.waitstatus_to_exitcode(status)
    return elapsed, exit_code, ru, stdout + "\n" + stderr


def build_worktree(tool, wt_tuple, shared_target_dir=None, is_workspace=True, extra_build_args=None):
    """`test --no-run` in one worktree. This is the measured phase."""
    i, wt_dir, _commit = wt_tuple
    env = _tool_env(tool, wt_dir, shared_target_dir)
    cmd = _test_cmd(tool, is_workspace, extra_build_args, no_run=True)
    elapsed, exit_code, ru, output = _run(cmd, wt_dir, env)
    res = {
        "wt_index": i,
        "build_wall_s": round(elapsed, 3),
        "exit_code": exit_code,
        "max_rss_mb": round(ru.ru_maxrss / 1024.0, 1),
        "build": parse_build_output(output, tool),
        "stderr_tail": output[-1500:] if exit_code != 0 else "",
    }
    if tool == "mitts_cas":
        res["mitts_stats"] = parse_mitts_stats(output)
        res.update(parse_mitts_io(output))
    return res


def smoke_worktree(tool, wt_tuple, shared_target_dir=None, is_workspace=True, extra_build_args=None):
    """
    Runs every built test binary with `--exact DEFECT_TEST_NAME`.
    Timed separately; never part of the metric.
    """
    i, wt_dir, _commit = wt_tuple
    env = _tool_env(tool, wt_dir, shared_target_dir)
    cmd = _test_cmd(tool, is_workspace, extra_build_args, no_run=False, filter_exact=DEFECT_TEST_NAME)
    elapsed, exit_code, _ru, output = _run(cmd, wt_dir, env)
    stats = parse_test_results(output)
    failed = extract_failed_tests(output) if exit_code != 0 else []
    return {
        "wt_index": i,
        "exec_wall_s": round(elapsed, 3),
        "exit_code": exit_code,
        "binaries_executed": stats["binaries_executed"],
        "passed": stats["passed"],
        "failed": stats["failed"],
        "failed_test_names": failed,
        "defect_reported": DEFECT_TEST_NAME in failed,
        "rebuilt": "Compiling" in output,
    }


# --------------------------------------------------------------------------- one swarm

def run_build_swarm(tool, worktrees, shared_target_dir, is_workspace, extra_build_args, preflight):
    loadavg_before, foreign_processes, preflight_waited_s = preflight
    target_path = worktrees[0][1]
    dirty_start_mb = sync_and_wait_dirty(max_dirty_mb=1024.0, timeout_s=120.0)
    dev_name, sec_before = get_device_sectors_written(target_path)

    start = time.perf_counter()
    sampler = SystemPressureSampler(interval_s=0.5)
    sampler.start()
    results = []
    with concurrent.futures.ThreadPoolExecutor(max_workers=len(worktrees)) as ex:
        futs = [ex.submit(build_worktree, tool, wt, shared_target_dir, is_workspace, extra_build_args) for wt in worktrees]
        for fut in concurrent.futures.as_completed(futs):
            try:
                results.append(fut.result())
            except Exception as e:  # a crashed worker is a failed worktree, never a missing one
                results.append({"wt_index": -1, "build_wall_s": -1, "exit_code": -1, "build": {"test_binaries_count": 0}, "stderr_tail": str(e)})
    pressure = sampler.stop()
    wall = round(time.perf_counter() - start, 3)

    dirty_end_mb = get_current_dirty_mb()
    sync_and_wait_dirty(max_dirty_mb=1024.0, timeout_s=120.0)
    _, sec_after = get_device_sectors_written(target_path)
    sectors = (sec_after - sec_before) if (sec_before is not None and sec_after is not None) else 0
    pressure.update({
        "dirty_at_start_mb": dirty_start_mb,
        "dirty_at_end_mb": dirty_end_mb,
        "device": dev_name,
        "device_sectors_written": sectors,
        "device_bytes_written": sectors * 512,
        "device_mb_written": round(sectors * 512 / (1024.0 * 1024.0), 1),
    })

    # Soundness probes, after the measured phase: clean WT-01 and defective WT-12.
    results.sort(key=lambda r: r["wt_index"])
    smoke = {
        "wt01": smoke_worktree(tool, worktrees[0], shared_target_dir, is_workspace, extra_build_args),
        "wt12": smoke_worktree(tool, worktrees[-1], shared_target_dir, is_workspace, extra_build_args),
    }
    _, loadavg_after, foreign_after = check_quiet_rig()
    return {
        "total_wall_s": wall,
        "results": results,
        "smoke": smoke,
        "system_pressure": pressure,
        "loadavg_before": loadavg_before,
        "foreign_processes": foreign_processes,
        "preflight_waited_s": preflight_waited_s,
        "loadavg_after": loadavg_after,
        "foreign_processes_after": foreign_after,
    }


# --------------------------------------------------------------------------- invariants

def _wt(sw, idx):
    for r in sw["results"]:
        if r["wt_index"] == idx:
            return r
    return {"exit_code": -1, "build": {"test_binaries_count": 0}}


def evaluate_invariants(tool_runs, fast_lock_noop, baseline_count):
    """All checks are over every run and every worktree; a single miss fails the flag."""
    builds_ok = True
    counts_match = True
    defect_caught = True
    clean_smoke_ok = True
    for run_i in range(len(tool_runs["cargo_bare"])):
        swarms = {t: tool_runs[t][run_i] for t in SOUND_TOOLS}
        for wt in range(1, 13):
            rows = {t: _wt(swarms[t], wt) for t in SOUND_TOOLS}
            if any(r["exit_code"] != 0 for r in rows.values()):
                builds_ok = False
            counts = {r["build"].get("test_binaries_count", 0) for r in rows.values()}
            if len(counts) != 1 or 0 in counts:
                counts_match = False
        for t in SOUND_TOOLS:
            s12 = swarms[t]["smoke"]["wt12"]
            if s12["exit_code"] == 0 or not s12["defect_reported"]:
                defect_caught = False
            s01 = swarms[t]["smoke"]["wt01"]
            built = _wt(swarms[t], 1)["build"].get("test_binaries_count", 0)
            if s01["exit_code"] != 0 or s01["failed"] != 0 or s01["binaries_executed"] != built or built == 0:
                clean_smoke_ok = False

    c_wt1 = [_wt(sw, 1)["build_wall_s"] for sw in tool_runs["cargo_bare"]]
    c_spread = spread_pct(c_wt1)

    sh_runs = tool_runs.get("cargo_shared", [])
    sh_wt12_exits = [sw["smoke"]["wt12"]["exit_code"] for sw in sh_runs]
    sh_clean_build_failures = [
        sum(1 for r in sw["results"] if r["wt_index"] < 12 and r["exit_code"] != 0) for sw in sh_runs
    ]
    sh_clean_smoke_failures = [sw["smoke"]["wt01"]["exit_code"] != 0 for sw in sh_runs]

    return {
        "build_exit_zero_all_worktrees_all_sound_arms": builds_ok,
        "test_binary_counts_match_all_sound_arms": counts_match,
        "defect_caught_wt12_all_sound_arms": defect_caught,
        "clean_wt01_smoke_ok_all_sound_arms": clean_smoke_ok,
        "cargo_bare_wt1_build_times": c_wt1,
        "cargo_bare_wt1_variance_pct": c_spread,
        "cargo_bare_wt1_variance_le_15_pct": c_spread <= 15.0,
        "fast_lock_noop_same_binary_count": (
            fast_lock_noop["exit_code"] == 0
            and fast_lock_noop["build"].get("test_binaries_count", 0) == baseline_count
            and baseline_count > 0
        ),
        "fast_lock_noop": {
            "exit_code": fast_lock_noop["exit_code"],
            "wall_s": fast_lock_noop["build_wall_s"],
            "test_binaries_count": fast_lock_noop["build"].get("test_binaries_count", 0),
            "mitts_stats": fast_lock_noop.get("mitts_stats"),
        },
        "test_binaries_count": baseline_count,
        "cargo_shared_unsound_empirical_confirmation": {
            "observed_unsound": any(ec == 0 for ec in sh_wt12_exits) or any(n > 0 for n in sh_clean_build_failures) or any(sh_clean_smoke_failures),
            "wt12_smoke_exit_codes": sh_wt12_exits,
            "clean_wt_build_failures_per_run": sh_clean_build_failures,
            "clean_wt01_smoke_failed_per_run": sh_clean_smoke_failures,
            "description": "cargo_shared shares one target directory across 12 worktrees on 4 commits; a missed defect in WT-12 or a failed clean worktree is the expected unsound behaviour",
        },
    }


# --------------------------------------------------------------------------- one project

def _summarise_swarm(idx, sw, tool, keep_targets):
    rows = []
    for r in sw["results"]:
        row = {
            "wt_index": r["wt_index"],
            "build_wall_s": r["build_wall_s"],
            "exit_code": r["exit_code"],
            "test_binaries_count": r["build"].get("test_binaries_count", 0),
            "max_rss_mb": r.get("max_rss_mb"),
        }
        if "cargo_reported_s" in r["build"]:
            row["cargo_reported_s"] = r["build"]["cargo_reported_s"]
        if keep_targets and r["wt_index"] == 1 and r["build"].get("targets"):
            row["targets"] = r["build"]["targets"]
        if tool == "mitts_cas":
            row["mitts_stats"] = r.get("mitts_stats")
            if "io_stats" in r:
                row["io_stats"] = r["io_stats"]
            if "timings_sum_s" in r:
                row["timings_sum_s"] = r["timings_sum_s"]
        if r.get("stderr_tail"):
            row["stderr_tail"] = r["stderr_tail"]
        rows.append(row)
    return {
        "run_idx": idx,
        "total_wall_s": sw["total_wall_s"],
        "loadavg_before": sw["loadavg_before"],
        "foreign_processes": sw["foreign_processes"],
        "preflight_waited_s": sw["preflight_waited_s"],
        "loadavg_after": sw["loadavg_after"],
        "foreign_processes_after": sw["foreign_processes_after"],
        "system_pressure": sw["system_pressure"],
        "worktrees": rows,
        "smoke": sw["smoke"],
    }


def run_t5_project(project_name, runs_count=5):
    print(f"\n=== T5 build swarm: {project_name} ===")
    repo_dir = PROJECTS_DIR / project_name
    if not repo_dir.exists():
        raise RuntimeError(f"Project repository not found: {repo_dir}")

    log = subprocess.check_output(["git", "log", "-n", "15", "--format=%H"], cwd=repo_dir, text=True).strip().splitlines()
    pick = lambda n: log[n] if len(log) > n else log[-1]
    commits = [pick(0)] * 4 + [pick(1)] * 3 + [pick(5)] * 3 + [pick(14)] * 2

    scratch = BENCH_ROOT / f"scratch_t5_{project_name}"
    wt_base, snap_cas, snap_sccache = scratch / "worktrees", scratch / "snap_cas", scratch / "snap_sccache"
    cache_root = BENCH_ROOT / "cache" if (BENCH_ROOT / "cache").exists() else Path.home() / ".cache"
    mitts_cache = Path(os.environ.get("MITTS_CACHE_DIR", cache_root / "mitts"))
    sccache_dir = Path(os.environ.get("SCCACHE_DIR", cache_root / "sccache"))
    os.environ["MITTS_CACHE_DIR"] = str(mitts_cache)
    os.environ["SCCACHE_DIR"] = str(sccache_dir)

    # Build-only arguments. tests-build is tokio's trybuild crate: it compiles
    # fixture programs at test *run* time and has no place in a build metric.
    extra_build_args = ["--exclude", "tests-build"] if project_name == "tokio" else []
    is_ws = True

    wait_for_quiet_rig(timeout_s=PREFLIGHT_TIMEOUT_S, max_load=PREFLIGHT_MAX_LOAD)
    initial_prov = get_provenance_info(mitts_cache, wt_base)
    initial_prov["extra_build_args"] = extra_build_args

    print("[1/5] Warming Mitts CAS on WT-01 (test --no-run)")
    init_wts = prepare_worktrees(repo_dir, wt_base, commits)
    if mitts_cache.exists():
        shutil.rmtree(mitts_cache, ignore_errors=True)
    clean_sccache(sccache_dir)
    warm = build_worktree("mitts_cas", init_wts[0], is_workspace=is_ws, extra_build_args=extra_build_args)
    if warm["exit_code"] != 0:
        raise RuntimeError(f"Mitts warm-up build failed (exit {warm['exit_code']}):\n{warm['stderr_tail']}")
    snapshot_cache(mitts_cache, snap_cas)

    print("[2/5] Warming sccache on WT-01 (test --no-run)")
    warm_s = build_worktree("cargo_sccache", init_wts[0], is_workspace=is_ws, extra_build_args=extra_build_args)
    if warm_s["exit_code"] != 0:
        raise RuntimeError(f"sccache warm-up build failed (exit {warm_s['exit_code']}):\n{warm_s['stderr_tail']}")
    snapshot_cache(sccache_dir, snap_sccache)
    subprocess.run([str(SCCACHE_BIN), "--stop-server"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

    tool_runs = {t: [] for t in TOOLS}
    disk_mb = {t: 0.0 for t in TOOLS}
    sccache_stats_runs = []

    print(f"[3/5] {runs_count} iterations x 4 arms x 12 worktrees")
    for run_idx in range(1, runs_count + 1):
        shift = (run_idx - 1) % len(TOOLS)
        order = TOOLS[shift:] + TOOLS[:shift]
        print(f"  --- iteration {run_idx}/{runs_count}: {', '.join(order)}")
        for tool in order:
            preflight = wait_for_quiet_rig(timeout_s=PREFLIGHT_TIMEOUT_S, max_load=PREFLIGHT_MAX_LOAD)
            wts = prepare_worktrees(repo_dir, wt_base, commits)
            inject_test_defect(wts[-1][1])
            shared_target = None
            if tool == "mitts_cas":
                restore_cache(snap_cas, mitts_cache)
            elif tool == "cargo_sccache":
                restore_cache(snap_sccache, sccache_dir)
                subprocess.run([str(SCCACHE_BIN), "--start-server"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                subprocess.run([str(SCCACHE_BIN), "--zero-stats"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            elif tool == "cargo_shared":
                shared_target = SHARED_TARGET_ROOT / f"{project_name}_test"
                shutil.rmtree(shared_target, ignore_errors=True)
                shared_target.mkdir(parents=True, exist_ok=True)

            sw = run_build_swarm(tool, wts, shared_target, is_ws, extra_build_args, preflight)
            tool_runs[tool].append(sw)

            wt_dirs = [d for _, d, _ in wts]
            if tool == "cargo_sccache":
                disk = get_disk_usage_mb(wt_dirs + [sccache_dir])
                try:
                    sccache_stats_runs.append(parse_sccache_stats(subprocess.check_output([str(SCCACHE_BIN), "--show-stats"], text=True)))
                except Exception:
                    sccache_stats_runs.append({})
                subprocess.run([str(SCCACHE_BIN), "--stop-server"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            elif tool == "mitts_cas":
                disk = get_disk_usage_mb([mitts_cache]) + get_disk_usage_mb(wt_dirs)
            elif tool == "cargo_shared":
                disk = get_disk_usage_mb(wt_dirs + [shared_target])
            else:
                disk = get_disk_usage_mb(wt_dirs)
            disk_mb[tool] = max(disk_mb[tool], disk)

            fails = sum(1 for r in sw["results"] if r["exit_code"] != 0)
            print(
                f"    {tool:13s} build {sw['total_wall_s']:8.2f}s | build failures {fails:2d} | "
                f"wt12 defect {'caught' if sw['smoke']['wt12']['defect_reported'] else 'MISSED':6s} | "
                f"wt01 smoke exit {sw['smoke']['wt01']['exit_code']} | dev_wr {sw['system_pressure'].get('device_mb_written', 0):.0f}MB | "
                f"load {sw['loadavg_before']}->{sw['loadavg_after']}"
            )

    print("[4/5] fast-lock no-op: `test --no-run` again on WT-01 with a warm store")
    fast_lock_noop = build_worktree("mitts_cas", (1, wt_base / "wt-01", commits[0]), is_workspace=is_ws, extra_build_args=extra_build_args)

    print("[5/5] invariants")
    baseline_count = _wt(tool_runs["cargo_bare"][0], 1)["build"].get("test_binaries_count", 0)
    invariants = evaluate_invariants(tool_runs, fast_lock_noop, baseline_count)

    runs_out = {}
    for tool in TOOLS:
        durations = [sw["total_wall_s"] for sw in tool_runs[tool]]
        entry = {"durations_s": durations, "disk_mb": disk_mb[tool]}
        entry.update(compute_stats(durations))
        entry["device_mb_written_stats"] = compute_stats([sw["system_pressure"].get("device_mb_written", 0.0) for sw in tool_runs[tool]])
        entry["per_wt_build_wall_stats"] = compute_stats([r["build_wall_s"] for sw in tool_runs[tool] for r in sw["results"] if r["build_wall_s"] > 0])
        entry["smoke_exec_wall_stats"] = compute_stats([sw["smoke"][k]["exec_wall_s"] for sw in tool_runs[tool] for k in ("wt01", "wt12")])
        entry["per_run_summary"] = [_summarise_swarm(i + 1, sw, tool, keep_targets=(i == 0)) for i, sw in enumerate(tool_runs[tool])]
        if tool == "mitts_cas":
            entry["mitts_stats_per_run"] = [
                {
                    "run_idx": i + 1,
                    "compiled": sum(r.get("mitts_stats", {}).get("compiled", 0) for r in sw["results"]),
                    "cached": sum(r.get("mitts_stats", {}).get("cached", 0) for r in sw["results"]),
                    "total": sum(r.get("mitts_stats", {}).get("total", 0) for r in sw["results"]),
                    "per_worktree": [{"wt_index": r["wt_index"], **(r.get("mitts_stats") or {})} for r in sw["results"]],
                }
                for i, sw in enumerate(tool_runs[tool])
            ]
        if tool == "cargo_sccache":
            entry["sccache_stats_per_run"] = sccache_stats_runs
        runs_out[tool] = entry

    bare_med, scc_med, mitts_med = runs_out["cargo_bare"]["median"], runs_out["cargo_sccache"]["median"], runs_out["mitts_cas"]["median"]
    sound_name = "cargo_sccache" if scc_med <= bare_med else "cargo_bare"
    speedup = {
        "sound_baseline": sound_name,
        "ratio": round(min(bare_med, scc_med) / max(mitts_med, 0.001), 2),
        "ratio_vs_bare": round(bare_med / max(mitts_med, 0.001), 2),
        "ratio_vs_sccache": round(scc_med / max(mitts_med, 0.001), 2),
    }
    print(f"{project_name}: mitts {mitts_med}s | bare {bare_med}s | sccache {scc_med}s | vs {sound_name} {speedup['ratio']}x")
    print(f"invariants: {json.dumps({k: v for k, v in invariants.items() if isinstance(v, bool)})}")

    return {
        "mode": "build",
        "metric": "wall-clock of 12 parallel `test --no-run` builds; test execution excluded",
        "worktrees_count": 12,
        "provenance": initial_prov,
        "commit_distribution": "4/3/3/2",
        "extra_build_args": extra_build_args,
        "smoke_filter": DEFECT_TEST_NAME,
        "runs": runs_out,
        "speedup_vs_sound_baseline": speedup,
        "invariants": invariants,
    }


# --------------------------------------------------------------------------- main

def main():
    parser = argparse.ArgumentParser(description="T5 multi-worktree build swarm, defensible audit")
    parser.add_argument("--projects", default="ripgrep,serde,tokio,nushell", help="comma-separated project list")
    parser.add_argument("--runs", type=int, default=5, help="iterations per project (default 5)")
    parser.add_argument("--output", default=None, help="output JSON path")
    args = parser.parse_args()

    out_p = Path(args.output) if args.output else (DEFAULT_RESULTS_DIR / "results_t5_build_swarm_defensible.json")
    report = {
        "schema_version": SCHEMA_VERSION,
        "provenance": get_provenance_info(),
        "rig": get_rig_info(),
        "mode": "build",
        "projects": {},
    }
    if out_p.exists():
        try:
            with open(out_p, "r") as f:
                report["projects"] = json.load(f).get("projects", {})
        except Exception:
            pass

    for proj in [p.strip() for p in args.projects.split(",") if p.strip()]:
        try:
            report["projects"][proj] = run_t5_project(proj, runs_count=args.runs)
        except Exception as e:
            import traceback
            traceback.print_exc()
            report["projects"][proj] = {"mode": "build", "status": "FAIL", "error": str(e), "provenance": get_provenance_info()}
        out_p.parent.mkdir(parents=True, exist_ok=True)
        with open(out_p, "w") as f:
            json.dump(report, f, indent=2)
        print(f"saved {out_p}")


if __name__ == "__main__":
    main()
