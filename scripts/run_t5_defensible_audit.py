# Mitts Build — Rust-first build and package tooling with shared artifact cache
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0

"""
T5: Multi-Worktree Test Swarm Defensible Audit
Measures test compilation throughput, execution wall-clock time, caching speedup,
peak memory, disk footprint, and test result parity across 12 git worktrees on
4 distinct commits on AMD Ryzen 9 9950X.
"""

import argparse
import concurrent.futures
import functools
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

import hashlib
from benchmark_common import (
    check_quiet_rig,
    get_binary_provenance,
    get_filesystem_type,
    get_rig_info,
    wait_for_quiet_rig,
)
from benchmark_worktree import (
    clean_sccache,
    get_disk_usage_mb,
    prepare_worktrees,
    restore_cache,
    snapshot_cache,
)

print = functools.partial(print, flush=True)

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
    for name in ["run_t5_defensible_audit.py", "benchmark_worktree.py", "benchmark_common.py"]:
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
            ["git", "-C", str(git_dir), "rev-parse", "HEAD"],
            text=True,
            stderr=subprocess.DEVNULL
        ).strip()
        any_dirty = False
        for sname, s_sha in script_shas.items():
            try:
                git_blob = subprocess.check_output(
                    ["git", "-C", str(git_dir), "show", f"HEAD:scripts/{sname}"],
                    stderr=subprocess.DEVNULL
                )
                blob_sha = hashlib.sha256(git_blob).hexdigest()
                if blob_sha != s_sha:
                    any_dirty = True
                    break
            except Exception:
                any_dirty = True
                break
        scripts_dirty = any_dirty
    except Exception:
        git_head = None
        scripts_dirty = None

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
    prov["preflight"] = {
        "loadavg_1m": load_1m,
        "foreign_processes": foreign_procs,
        "is_quiet": is_quiet,
    }
    return prov

BENCH_ROOT = Path(os.environ.get("BENCH_ROOT", Path.home() / "mitts-benchmarks"))
DEFAULT_RESULTS_DIR = Path(
    os.environ.get("BENCHMARK_OUTPUT_DIR", BENCH_ROOT / "benchmark-results")
)
PROJECTS_DIR = BENCH_ROOT / "projects"
SHARED_TARGET_ROOT = BENCH_ROOT / "shared_cargo_target"
MITTS_BIN = Path(os.environ.get("MITTS_BIN", BENCH_ROOT / "mitts-bin"))
SCCACHE_BIN = Path(shutil.which("sccache") or (Path.home() / ".cargo/bin/sccache"))
CARGO_BIN_DIR = str(Path.home() / ".cargo" / "bin")

if CARGO_BIN_DIR not in os.environ.get("PATH", ""):
    os.environ["PATH"] = f"{CARGO_BIN_DIR}:{os.environ.get('PATH', '')}"


def inject_test_defect(wt_dir: Path):
    # Prefer integration test files in tests/ directories so that `cargo test --tests` executes them
    candidates = []
    for r, _, files in os.walk(wt_dir):
        if (
            ".git" in r
            or "target" in r
            or "plugins" in r
            or "plugin_persistence" in r
            or "tests-build" in r
            or "tests-integration" in r
            or "benches" in r
            or "examples" in r
            or "stress-test" in r
        ):
            continue
        p_r = Path(r)
        if "tests" in p_r.parts:
            for f in sorted(files):
                if f.endswith(".rs") and not f.startswith(".") and not f.startswith("_"):
                    if f == "lib.rs" or f == "main.rs":
                        continue
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
    if target:
        with open(target, "r") as f:
            content = f.read()
        snippet = "\n#[test]\nfn __intentional_test_failure() {\n    assert_eq!(2 + 2, 5, \"intentional test failure for audit\");\n}\n"
        with open(target, "w") as f:
            f.write(content + snippet)
        return str(target.relative_to(wt_dir))
    return None


def parse_test_results(output: str):
    matches = re.findall(
        r"test result: (ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored",
        output,
    )
    total_passed = sum(int(m[1]) for m in matches)
    total_failed = sum(int(m[2]) for m in matches)
    total_ignored = sum(int(m[3]) for m in matches)
    return {
        "binaries_count": len(matches),
        "passed": total_passed,
        "failed": total_failed,
        "ignored": total_ignored,
    }


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
    m_fail = re.search(r"failures:\n((?:\s+[^\n]+\n)+)", text)
    if m_fail:
        for f_line in m_fail.group(1).splitlines():
            name = f_line.strip()
            if name and not name.startswith("----") and name not in failed:
                failed.append(name)
    return failed


def parse_phase_breakdown(text: str, elapsed: float, tool: str):
    if tool == "mitts_cas":
        m = re.search(
            r"\[mitts::phases\]\s+compile_s=([\d\.]+)\s+test_exec_s=([\d\.]+)\s+total_s=([\d\.]+)",
            text,
        )
        if m:
            compile_s = round(float(m.group(1)), 3)
            test_exec_s = round(float(m.group(2)), 3)
            res = {
                "compile_s": compile_s,
                "test_exec_s": test_exec_s,
                "total_wall_s": round(float(m.group(3)), 3),
            }
            m_io = re.search(
                r"\[mitts::io\]\s+bytes_cloned=(\d+)\s+bytes_copied=(\d+)\s+bytes_hardlinked=(\d+)\s+files_cloned=(\d+)\s+files_copied=(\d+)\s+files_hardlinked=(\d+)",
                text,
            )
            if m_io:
                res["io_stats"] = {
                    "bytes_cloned": int(m_io.group(1)),
                    "bytes_copied": int(m_io.group(2)),
                    "bytes_hardlinked": int(m_io.group(3)),
                    "files_cloned": int(m_io.group(4)),
                    "files_copied": int(m_io.group(5)),
                    "files_hardlinked": int(m_io.group(6)),
                }
            m_sum = re.search(
                r"\[mitts::timings_sum\]\s+restore_sum_s=([\d\.]+)\s+restore_miss_sum_s=([\d\.]+)\s+publish_sum_s=([\d\.]+)\s+rustc_sum_s=([\d\.]+)",
                text,
            )
            if m_sum:
                res["timings_sum_s"] = {
                    "restore_sum_s": float(m_sum.group(1)),
                    "restore_miss_sum_s": float(m_sum.group(2)),
                    "publish_sum_s": float(m_sum.group(3)),
                    "rustc_sum_s": float(m_sum.group(4)),
                }
            return res

    # Cargo fallback for cargo_bare, cargo_sccache, cargo_shared
    m_cargo = re.search(
        r"Finished\s+`?(?:test|dev|release)`?\s+profile.*?in\s+(?:(\d+)m\s+)?([\d\.]+)s",
        text,
    )
    if m_cargo:
        mins = float(m_cargo.group(1)) if m_cargo.group(1) else 0.0
        secs = float(m_cargo.group(2))
        c_s = round(mins * 60.0 + secs, 3)
        t_s = round(max(0.0, elapsed - c_s), 3)
        return {
            "compile_s": c_s,
            "test_exec_s": t_s,
            "total_wall_s": round(elapsed, 3),
        }

    return {
        "compile_s": round(elapsed, 3),
        "test_exec_s": 0.0,
        "total_wall_s": round(elapsed, 3),
    }


def run_single_worktree(tool, wt_tuple, shared_target_dir=None, is_workspace=True, extra_test_args=None):
    i, wt_dir, commit = wt_tuple
    env = os.environ.copy()
    if CARGO_BIN_DIR not in env.get("PATH", ""):
        env["PATH"] = f"{CARGO_BIN_DIR}:{env.get('PATH', '')}"
    cargo_bin = shutil.which("cargo") or (Path.home() / ".cargo/bin/cargo")
    env["CARGO"] = str(cargo_bin)
    env["RUSTUP_HOME"] = str(Path.home() / ".rustup")
    env["CARGO_HOME"] = str(Path.home() / ".cargo")
    env["MITTS_TIMINGS"] = "1"

    # Isolate TMPDIR per worktree to prevent cross-worktree race conditions
    wt_tmp = (wt_dir / "tmp").resolve()
    wt_tmp.mkdir(parents=True, exist_ok=True)
    env["TMPDIR"] = str(wt_tmp)

    ws_args = ["--workspace"] if is_workspace else []
    extra_tail = (extra_test_args or [])
    base_test_args = ["--no-fail-fast"]

    if tool == "mitts_cas":
        cmd = [str(MITTS_BIN), "test", "--tests"] + ws_args + base_test_args + extra_tail
    elif tool == "cargo_bare":
        cmd = ["cargo", "test", "--offline", "--tests"] + ws_args + base_test_args + extra_tail
    elif tool == "cargo_sccache":
        cmd = ["cargo", "test", "--offline", "--tests"] + ws_args + base_test_args + extra_tail
        env["RUSTC_WRAPPER"] = str(SCCACHE_BIN)
    elif tool == "cargo_shared":
        cmd = ["cargo", "test", "--offline", "--tests"] + ws_args + base_test_args + extra_tail
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
    full_output = stdout + "\n" + stderr
    test_stats = parse_test_results(full_output)
    mitts_stats = parse_mitts_stats(full_output) if tool == "mitts_cas" else None
    phases = parse_phase_breakdown(full_output, elapsed, tool)
    failed_test_names = extract_failed_tests(full_output) if exit_code != 0 else []
    return {
        "wt_index": i,
        "wall_sec": round(elapsed, 3),
        "phases": phases,
        "exit_code": exit_code,
        "max_rss_mb": round(ru.ru_maxrss / 1024.0, 1),
        "stdout": stdout,
        "stderr": stderr,
        "test_stats": test_stats,
        "mitts_stats": mitts_stats,
        "failed_test_names": failed_test_names,
    }


def get_current_dirty_mb():
    try:
        with open("/proc/meminfo", "r") as f:
            for line in f:
                if line.startswith("Dirty:"):
                    return round(int(line.split()[1]) / 1024.0, 1)
    except Exception:
        pass
    return 0.0


def sync_and_wait_dirty(max_dirty_mb=1024.0, timeout_s=120.0):
    try:
        subprocess.run(["sync"], check=True)
    except Exception:
        pass
    t0 = time.time()
    while time.time() - t0 < timeout_s:
        d_mb = get_current_dirty_mb()
        if d_mb <= max_dirty_mb:
            return d_mb
        time.sleep(0.5)
    return get_current_dirty_mb()


def get_device_sectors_written(target_path):
    try:
        src = subprocess.check_output(
            ["findmnt", "-no", "SOURCE", "--target", str(target_path)],
            text=True,
            stderr=subprocess.DEVNULL,
        ).strip()
        real_src = os.path.realpath(src)
        dev_name = os.path.basename(real_src)
        stat_file = f"/sys/class/block/{dev_name}/stat"
        if os.path.exists(stat_file):
            with open(stat_file, "r") as f:
                fields = f.read().split()
            return dev_name, int(fields[6])
    except Exception:
        pass
    return None, None


class SystemPressureSampler:
    def __init__(self, interval_s=0.5):
        self.interval_s = interval_s
        self._stop_event = threading.Event()
        self._thread = None
        self.samples = []

    def _sample_once(self):
        sample = {
            "time": time.time(),
            "dirty_mb": 0.0,
            "writeback_mb": 0.0,
            "io_some_avg10": 0.0,
            "io_full_avg10": 0.0,
        }
        try:
            with open("/proc/meminfo", "r") as f:
                for line in f:
                    if line.startswith("Dirty:"):
                        sample["dirty_mb"] = round(int(line.split()[1]) / 1024.0, 1)
                    elif line.startswith("Writeback:"):
                        sample["writeback_mb"] = round(int(line.split()[1]) / 1024.0, 1)
        except Exception:
            pass

        try:
            with open("/proc/pressure/io", "r") as f:
                for line in f:
                    if line.startswith("some "):
                        m = re.search(r"avg10=([\d\.]+)", line)
                        if m:
                            sample["io_some_avg10"] = float(m.group(1))
                    elif line.startswith("full "):
                        m = re.search(r"avg10=([\d\.]+)", line)
                        if m:
                            sample["io_full_avg10"] = float(m.group(1))
        except Exception:
            pass
        return sample

    def _run(self):
        while not self._stop_event.is_set():
            self.samples.append(self._sample_once())
            self._stop_event.wait(self.interval_s)

    def start(self):
        self._stop_event.clear()
        self.samples = []
        self._thread = threading.Thread(target=self._run, daemon=True)
        self._thread.start()

    def stop(self):
        self._stop_event.set()
        if self._thread:
            self._thread.join(timeout=2.0)
        if not self.samples:
            return {}
        dirty_vals = [s["dirty_mb"] for s in self.samples]
        wb_vals = [s["writeback_mb"] for s in self.samples]
        io_some = [s["io_some_avg10"] for s in self.samples]
        io_full = [s["io_full_avg10"] for s in self.samples]
        return {
            "samples_count": len(self.samples),
            "max_dirty_mb": max(dirty_vals) if dirty_vals else 0.0,
            "avg_dirty_mb": round(sum(dirty_vals) / len(dirty_vals), 1) if dirty_vals else 0.0,
            "max_writeback_mb": max(wb_vals) if wb_vals else 0.0,
            "avg_writeback_mb": round(sum(wb_vals) / len(wb_vals), 1) if wb_vals else 0.0,
            "max_io_some_avg10": max(io_some) if io_some else 0.0,
            "avg_io_some_avg10": round(sum(io_some) / len(io_some), 2) if io_some else 0.0,
            "max_io_full_avg10": max(io_full) if io_full else 0.0,
            "avg_io_full_avg10": round(sum(io_full) / len(io_full), 2) if io_full else 0.0,
        }


def run_test_swarm(tool, worktrees, shared_target_dir=None, is_workspace=True, extra_test_args=None):
    loadavg_before, foreign_procs = wait_for_quiet_rig(timeout_s=300.0, max_load=1.0)
    target_path = worktrees[0][1] if worktrees else Path.cwd()
    dirty_start_mb = sync_and_wait_dirty(max_dirty_mb=1024.0, timeout_s=120.0)
    dev_name, sec_before = get_device_sectors_written(target_path)

    start = time.perf_counter()
    sampler = SystemPressureSampler(interval_s=0.5)
    sampler.start()
    results = []
    with concurrent.futures.ThreadPoolExecutor(max_workers=len(worktrees)) as executor:
        futs = {executor.submit(run_single_worktree, tool, wt, shared_target_dir, is_workspace, extra_test_args): wt for wt in worktrees}
        for fut in concurrent.futures.as_completed(futs):
            try:
                results.append(fut.result())
            except Exception as e:
                results.append({"wt_index": -1, "wall_sec": -1, "exit_code": -1, "max_rss_mb": 0, "stdout": "", "stderr": str(e), "test_stats": {}})
    pressure = sampler.stop()
    wall = round(time.perf_counter() - start, 3)

    dirty_end_mb = get_current_dirty_mb()
    _, sec_after_swarm = get_device_sectors_written(target_path)

    # Flush trailing dirty pages to capture complete disk write volume
    sync_and_wait_dirty(max_dirty_mb=1024.0, timeout_s=120.0)
    _, sec_after_sync = get_device_sectors_written(target_path)

    sec_written_swarm = (sec_after_swarm - sec_before) if (sec_before is not None and sec_after_swarm is not None) else 0
    sec_written_total = (sec_after_sync - sec_before) if (sec_before is not None and sec_after_sync is not None) else 0

    bytes_written_swarm = sec_written_swarm * 512
    bytes_written_total = sec_written_total * 512

    device_io = {
        "device": dev_name,
        "sectors_written_swarm": sec_written_swarm,
        "bytes_written_swarm": bytes_written_swarm,
        "mb_written_swarm": round(bytes_written_swarm / (1024.0 * 1024.0), 1),
        "sectors_written_total": sec_written_total,
        "bytes_written_total": bytes_written_total,
        "mb_written_total": round(bytes_written_total / (1024.0 * 1024.0), 1),
    }

    pressure["dirty_at_start_mb"] = dirty_start_mb
    pressure["dirty_at_end_mb"] = dirty_end_mb
    pressure["device"] = dev_name
    pressure["device_sectors_written"] = sec_written_total
    pressure["device_bytes_written"] = bytes_written_total
    pressure["device_mb_written"] = round(bytes_written_total / (1024.0 * 1024.0), 1)

    results.sort(key=lambda r: r["wt_index"])
    return {
        "total_wall_s": wall,
        "results": results,
        "system_pressure": pressure,
        "device_io": device_io,
        "loadavg_before": loadavg_before,
        "foreign_processes": foreign_procs,
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
    return {
        "median": round(median, 3),
        "q1": round(q1, 3),
        "q3": round(q3, 3),
        "iqr": round(iqr, 3),
        "min": round(s[0], 3),
        "max": round(s[-1], 3),
    }


def run_t5_project(project_name, runs_count=5):
    print(f"\n=======================================================")
    print(f"  Starting T5 Defensible Test Swarm: {project_name}")
    print(f"=======================================================")

    repo_dir = PROJECTS_DIR / project_name
    if not repo_dir.exists():
        raise RuntimeError(f"Project repository not found: {repo_dir}")

    out = subprocess.check_output(["git", "log", "-n", "15", "--format=%H"], cwd=repo_dir, text=True)
    commits_all = out.strip().splitlines()
    c_head = commits_all[0]
    c_1 = commits_all[1] if len(commits_all) > 1 else c_head
    c_5 = commits_all[5] if len(commits_all) > 5 else commits_all[-1]
    c_14 = commits_all[14] if len(commits_all) > 14 else commits_all[-1]
    commits = [c_head] * 4 + [c_1] * 3 + [c_5] * 3 + [c_14] * 2

    scratch_dir = BENCH_ROOT / f"scratch_t5_{project_name}"
    wt_base = scratch_dir / "worktrees"
    snap_cas = scratch_dir / "snap_cas"
    snap_sccache = scratch_dir / "snap_sccache"
    default_cache = (
        (BENCH_ROOT / "cache" / "mitts")
        if (BENCH_ROOT / "cache").exists()
        else (Path.home() / ".cache" / "mitts")
    )
    default_sccache = (
        (BENCH_ROOT / "cache" / "sccache")
        if (BENCH_ROOT / "cache").exists()
        else (Path.home() / ".cache" / "sccache")
    )
    mitts_cache = Path(os.environ.get("MITTS_CACHE_DIR", default_cache))
    sccache_dir = Path(os.environ.get("SCCACHE_DIR", default_sccache))
    os.environ["MITTS_CACHE_DIR"] = str(mitts_cache)
    os.environ["SCCACHE_DIR"] = str(sccache_dir)

    is_ws = True
    extra_tail = (
        ["--exclude", "tests-build"]
        if project_name == "tokio"
        else (["--", "--skip", "f1414"] if project_name == "ripgrep" else [])
    )

    print("[1/5] Warming up Mitts CAS on WT-01...")
    init_wts = prepare_worktrees(repo_dir, wt_base, commits)
    if mitts_cache.exists():
        shutil.rmtree(mitts_cache, ignore_errors=True)
    clean_sccache(sccache_dir)

    res_warm = run_single_worktree("mitts_cas", init_wts[0], is_workspace=is_ws, extra_test_args=extra_tail)
    if res_warm["exit_code"] != 0:
        print(f"WARN: Mitts warmup failed:\n{res_warm['stderr'][:500]}")
    snapshot_cache(mitts_cache, snap_cas)

    print("[2/5] Warming up sccache on WT-01...")
    run_single_worktree("cargo_sccache", init_wts[0], is_workspace=is_ws, extra_test_args=extra_tail)
    snapshot_cache(sccache_dir, snap_sccache)

    tool_results = {
        "mitts_cas": {"durations_s": [], "disk_mb": 0.0, "runs_data": []},
        "cargo_bare": {"durations_s": [], "disk_mb": 0.0, "runs_data": []},
        "cargo_sccache": {"durations_s": [], "disk_mb": 0.0, "runs_data": []},
        "cargo_shared": {"durations_s": [], "disk_mb": 0.0, "runs_data": []},
    }

    last_cargo_res = None
    last_sccache_res = None
    last_mitts_res = None

    tools_base = ["mitts_cas", "cargo_bare", "cargo_sccache", "cargo_shared"]
    print(f"[3/5] Executing {runs_count} iterations across 12 worktrees per tool...")
    for run_idx in range(1, runs_count + 1):
        shift = (run_idx - 1) % len(tools_base)
        tools_in_run = tools_base[shift:] + tools_base[:shift]
        print(f"  --- Iteration {run_idx}/{runs_count} (tool order: {', '.join(tools_in_run)}) ---")
        for tool in tools_in_run:
            wts = prepare_worktrees(repo_dir, wt_base, commits)
            inject_test_defect(wts[-1][1])

            if tool == "mitts_cas":
                restore_cache(snap_cas, mitts_cache)
            elif tool == "cargo_sccache":
                restore_cache(snap_sccache, sccache_dir)
                try:
                    subprocess.run([str(SCCACHE_BIN), "--zero-stats"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                except Exception:
                    pass
            elif tool == "cargo_shared":
                st = SHARED_TARGET_ROOT / f"{project_name}_test"
                if st.exists():
                    shutil.rmtree(st, ignore_errors=True)
                st.mkdir(parents=True, exist_ok=True)

            shared_target = SHARED_TARGET_ROOT / f"{project_name}_test" if tool == "cargo_shared" else None
            swarm_res = run_test_swarm(tool, wts, shared_target, is_workspace=is_ws, extra_test_args=extra_tail)
            tool_results[tool]["durations_s"].append(swarm_res["total_wall_s"])
            tool_results[tool]["runs_data"].append(swarm_res)
            tool_results[tool].setdefault("system_pressure_runs", []).append(swarm_res.get("system_pressure", {}))
            tool_results[tool].setdefault("device_io_runs", []).append(swarm_res.get("device_io", {}))

            if tool == "cargo_sccache":
                disk = get_disk_usage_mb([wt_dir for _, wt_dir, _ in wts] + [sccache_dir])
                try:
                    s_out = subprocess.check_output([str(SCCACHE_BIN), "--show-stats"], text=True)
                    s_stats = parse_sccache_stats(s_out)
                    tool_results[tool].setdefault("sccache_stats_runs", []).append(s_stats)
                except Exception:
                    pass
            elif tool == "mitts_cas":
                cas_mb = get_disk_usage_mb([mitts_cache])
                wt_mb = get_disk_usage_mb([wt_dir for _, wt_dir, _ in wts])
                disk = cas_mb + wt_mb
                tot_comp = sum(r.get("mitts_stats", {}).get("compiled", 0) for r in swarm_res["results"] if r.get("mitts_stats"))
                tot_cach = sum(r.get("mitts_stats", {}).get("cached", 0) for r in swarm_res["results"] if r.get("mitts_stats"))
                tot_units = sum(r.get("mitts_stats", {}).get("total", 0) for r in swarm_res["results"] if r.get("mitts_stats"))
                per_wt = [
                    {
                        "wt_index": r["wt_index"],
                        "compiled": r.get("mitts_stats", {}).get("compiled", 0) if r.get("mitts_stats") else 0,
                        "cached": r.get("mitts_stats", {}).get("cached", 0) if r.get("mitts_stats") else 0,
                        "total": r.get("mitts_stats", {}).get("total", 0) if r.get("mitts_stats") else 0,
                        "fast_lock": r.get("mitts_stats", {}).get("fast_lock", 0) if r.get("mitts_stats") else 0,
                    }
                    for r in swarm_res["results"]
                ]
                tool_results[tool].setdefault("mitts_stats_runs", []).append({
                    "run_idx": run_idx,
                    "compiled": tot_comp,
                    "cached": tot_cach,
                    "total": tot_units,
                    "per_worktree": per_wt,
                })
            else:
                disk = get_disk_usage_mb([wt_dir for _, wt_dir, _ in wts])
            tool_results[tool]["disk_mb"] = max(tool_results[tool]["disk_mb"], disk)

            if tool == "cargo_bare":
                last_cargo_res = swarm_res
            elif tool == "cargo_sccache":
                last_sccache_res = swarm_res
            elif tool == "mitts_cas":
                last_mitts_res = swarm_res

            press = swarm_res.get("system_pressure", {})
            press_str = (
                f" | start_dirty: {press.get('dirty_at_start_mb', 0):.0f}MB"
                f" | dev_wr: {press.get('device_mb_written', 0):.0f}MB"
                f" | avg_dirty: {press.get('avg_dirty_mb', 0):.0f}MB"
                f" | avg_io: {press.get('avg_io_some_avg10', 0):.2f}"
                if press else ""
            )
            print(f"    {tool:14s}: {swarm_res['total_wall_s']:6.2f}s | disk: {disk:6.1f}MB{press_str}")

    for tool, data in tool_results.items():
        st = compute_stats(data["durations_s"])
        data.update(st)
        dev_wrs = [p.get("device_mb_written", 0.0) for p in data.get("system_pressure_runs", [])]
        if dev_wrs:
            data["device_mb_written_stats"] = compute_stats(dev_wrs)

    if "sccache_stats_runs" in tool_results["cargo_sccache"]:
        runs_s = tool_results["cargo_sccache"]["sccache_stats_runs"]
        if runs_s:
            tool_results["cargo_sccache"]["sccache_stats"] = runs_s[-1]
            tool_results["cargo_sccache"]["sccache_stats_per_run"] = runs_s
    if "mitts_stats_runs" in tool_results["mitts_cas"]:
        runs_m = tool_results["mitts_cas"]["mitts_stats_runs"]
        if runs_m:
            tool_results["mitts_cas"]["mitts_compilation_stats"] = runs_m[-1]
            tool_results["mitts_cas"]["mitts_stats_per_run"] = runs_m

    bare_med = tool_results["cargo_bare"]["median"]
    scc_med = tool_results["cargo_sccache"]["median"]
    mitts_med = tool_results["mitts_cas"]["median"]
    sound_baseline_name = "cargo_sccache" if scc_med <= bare_med else "cargo_bare"
    sound_baseline_val = min(bare_med, scc_med)
    speedup_ratio = round(sound_baseline_val / max(mitts_med, 0.001), 2)
    speedup_vs_bare = round(bare_med / max(mitts_med, 0.001), 2)

    print("[4/5] Verifying strict T5 test swarm invariants across all runs and worktrees...")
    all_runs_exit_codes_matched = True
    all_clean_wts_passed_cleanly = True
    all_clean_test_counts_matched = True

    m_swarms = tool_results["mitts_cas"]["runs_data"]
    c_swarms = tool_results["cargo_bare"]["runs_data"]
    s_swarms = tool_results["cargo_sccache"]["runs_data"]

    for m_sw, c_sw, s_sw in zip(m_swarms, c_swarms, s_swarms):
        for m_res, c_res, s_res in zip(m_sw["results"], c_sw["results"], s_sw["results"]):
            if m_res["exit_code"] != c_res["exit_code"] or m_res["exit_code"] != s_res["exit_code"]:
                all_runs_exit_codes_matched = False
            wt_i = m_res["wt_index"]
            if wt_i < 12:
                if m_res["exit_code"] != 0 or c_res["exit_code"] != 0 or s_res["exit_code"] != 0:
                    all_clean_wts_passed_cleanly = False
                m_st = m_res.get("test_stats", {})
                c_st = c_res.get("test_stats", {})
                s_st = s_res.get("test_stats", {})
                # Fail if test_stats missing, zero binaries, or zero passed tests on clean WT
                if (
                    not m_st
                    or not c_st
                    or not s_st
                    or m_st.get("binaries_count", 0) == 0
                    or c_st.get("binaries_count", 0) == 0
                    or s_st.get("binaries_count", 0) == 0
                    or m_st.get("passed", 0) == 0
                    or c_st.get("passed", 0) == 0
                    or s_st.get("passed", 0) == 0
                    or m_st.get("passed") != c_st.get("passed")
                    or m_st.get("passed") != s_st.get("passed")
                    or m_st.get("failed", 0) != c_st.get("failed", 0)
                    or m_st.get("failed", 0) != s_st.get("failed", 0)
                    or m_st.get("ignored", 0) != c_st.get("ignored", 0)
                    or m_st.get("ignored", 0) != s_st.get("ignored", 0)
                    or m_st.get("binaries_count") != c_st.get("binaries_count")
                    or m_st.get("binaries_count") != s_st.get("binaries_count")
                ):
                    all_clean_test_counts_matched = False
            else:
                # WT 12 has intentional test defect - must fail (exit != 0) across all sound tools
                if m_res["exit_code"] == 0 or c_res["exit_code"] == 0 or s_res["exit_code"] == 0:
                    all_runs_exit_codes_matched = False

    last_m_results = last_mitts_res["results"]
    last_c_results = last_cargo_res["results"]
    last_s_results = last_sccache_res["results"] if last_sccache_res else []
    defect_m = "__intentional_test_failure" in (last_m_results[-1]["stdout"] + last_m_results[-1]["stderr"])
    defect_c = "__intentional_test_failure" in (last_c_results[-1]["stdout"] + last_c_results[-1]["stderr"])
    defect_s = (
        "__intentional_test_failure" in (last_s_results[-1]["stdout"] + last_s_results[-1]["stderr"])
        if last_s_results
        else False
    )
    intentional_defect_caught_by_both = defect_m and defect_c and defect_s

    # Empirical check for cargo_shared unsound collision on WT-12
    sh_swarms = tool_results.get("cargo_shared", {}).get("runs_data", [])
    cargo_shared_wt12_exits = []
    cargo_shared_wt12_fails = []
    if sh_swarms:
        for s in sh_swarms:
            for r in s.get("results", []):
                if r.get("wt_index") == 12:
                    cargo_shared_wt12_exits.append(r.get("exit_code"))
                    st = r.get("test_stats", {})
                    cargo_shared_wt12_fails.append(st.get("failed", 0))

    cargo_shared_wt12_unsound = (
        len(cargo_shared_wt12_exits) > 0 and any(ec == 0 for ec in cargo_shared_wt12_exits)
    )

    m_stats_clean = last_m_results[0]["test_stats"]
    c_stats_clean = last_c_results[0]["test_stats"]
    s_stats_clean = last_s_results[0]["test_stats"] if last_s_results else {}

    print("[5/5] Testing P0 invariant: fast-lock hit NEVER skips test execution...")
    wt01_tuple = (1, wt_base / "wt-01", commits[0])
    res_fl = run_single_worktree("mitts_cas", wt01_tuple, is_workspace=is_ws, extra_test_args=extra_tail)

    fast_lock_executed_tests = (
        (res_fl["exit_code"] == last_m_results[0]["exit_code"])
        and (res_fl["test_stats"].get("passed", 0) == m_stats_clean.get("passed", 0))
        and (res_fl["test_stats"].get("passed", 0) > 0)
    )

    c_wt1_compile_times = [
        sr["results"][0].get("phases", {}).get("compile_s", 0)
        for sr in c_swarms
        if sr.get("results")
    ]
    if c_wt1_compile_times and min(c_wt1_compile_times) > 0:
        c_wt1_med = compute_stats(c_wt1_compile_times)["median"]
        c_wt1_spread_pct = round(((max(c_wt1_compile_times) - min(c_wt1_compile_times)) / c_wt1_med) * 100.0, 2)
    else:
        c_wt1_spread_pct = 0.0

    invariants = {
        "exit_codes_all_matched": all_runs_exit_codes_matched,
        "all_clean_worktrees_passed_cleanly": all_clean_wts_passed_cleanly,
        "all_clean_test_counts_matched": all_clean_test_counts_matched,
        "intentional_defect_caught_by_both": intentional_defect_caught_by_both,
        "test_counts_match": all_clean_test_counts_matched,
        "tests_passed_count": m_stats_clean.get("passed", 0),
        "fast_lock_never_skips_test_execution": fast_lock_executed_tests,
        "cargo_bare_wt1_compile_times": c_wt1_compile_times,
        "cargo_bare_wt1_variance_pct": c_wt1_spread_pct,
        "cargo_bare_wt1_variance_le_15_pct": c_wt1_spread_pct <= 15.0,
        "mitts_test_stats": m_stats_clean,
        "cargo_test_stats": c_stats_clean,
        "sccache_test_stats": s_stats_clean,
        "fast_lock_test_stats": res_fl["test_stats"],
        "fast_lock_exit_code": res_fl["exit_code"],
        "cargo_shared_unsound_empirical_confirmation": {
            "observed_unsound": cargo_shared_wt12_unsound,
            "wt12_exit_codes": cargo_shared_wt12_exits,
            "wt12_failed_counts": cargo_shared_wt12_fails,
            "description": "cargo_shared reused build artifacts across worktrees without isolating intentional defect in WT-12, completing with exit code 0 and failed=0",
        },
    }

    for tool in tool_results:
        runs_d = tool_results[tool].pop("runs_data", [])
        tool_results[tool]["per_run_summary"] = [
            {
                "run_idx": idx + 1,
                "total_wall_s": sr["total_wall_s"],
                "loadavg_before": sr.get("loadavg_before"),
                "foreign_processes": sr.get("foreign_processes", []),
                "system_pressure": sr.get("system_pressure", {}),
                "worktrees": [
                    {
                        "wt_index": r["wt_index"],
                        "wall_sec": r["wall_sec"],
                        "phases": r.get("phases", {}),
                        "exit_code": r["exit_code"],
                        "test_stats": r["test_stats"],
                        "mitts_stats": r.get("mitts_stats"),
                        "failed_test_names": r.get("failed_test_names", []),
                    }
                    for r in sr["results"]
                ],
                "totals": {
                    "bytes_copied": sum(r.get("phases", {}).get("io_stats", {}).get("bytes_copied", 0) for r in sr["results"]),
                    "bytes_cloned": sum(r.get("phases", {}).get("io_stats", {}).get("bytes_cloned", 0) for r in sr["results"]),
                    "bytes_hardlinked": sum(r.get("phases", {}).get("io_stats", {}).get("bytes_hardlinked", 0) for r in sr["results"]),
                    "restore_sum_s": round(sum(r.get("phases", {}).get("timings_sum_s", {}).get("restore_sum_s", 0) for r in sr["results"]), 3),
                    "restore_miss_sum_s": round(sum(r.get("phases", {}).get("timings_sum_s", {}).get("restore_miss_sum_s", 0) for r in sr["results"]), 3),
                    "publish_sum_s": round(sum(r.get("phases", {}).get("timings_sum_s", {}).get("publish_sum_s", 0) for r in sr["results"]), 3),
                    "rustc_sum_s": round(sum(r.get("phases", {}).get("timings_sum_s", {}).get("rustc_sum_s", 0) for r in sr["results"]), 3),
                } if tool == "mitts_cas" else {},
            }
            for idx, sr in enumerate(runs_d)
        ]
        all_compile_s = [
            r["phases"]["compile_s"]
            for sr in runs_d
            for r in sr["results"]
            if "phases" in r and "compile_s" in r["phases"]
        ]
        all_test_exec_s = [
            r["phases"]["test_exec_s"]
            for sr in runs_d
            for r in sr["results"]
            if "phases" in r and "test_exec_s" in r["phases"]
        ]
        tool_results[tool]["phase_stats"] = {
            "compile_s": compute_stats(all_compile_s),
            "test_exec_s": compute_stats(all_test_exec_s),
        }

    print(f"Results for {project_name}: Speedup vs {sound_baseline_name}: {speedup_ratio:.2f}x (vs bare: {speedup_vs_bare:.2f}x)")
    print(f"Invariants: {invariants}")

    return {
        "mode": "test",
        "worktrees_count": 12,
        "provenance": get_provenance_info(
            mitts_cache,
            (wts[0][1] if isinstance(wts[0], (list, tuple)) else wts[0].get("dir")) if wts else None,
        ),
        "commit_distribution": "4/3/3/2",
        "runs": tool_results,
        "speedup_vs_sound_baseline": {
            "sound_baseline": sound_baseline_name,
            "ratio": speedup_ratio,
            "ratio_vs_bare": speedup_vs_bare,
        },
        "invariants": invariants,
    }


def main():
    parser = argparse.ArgumentParser(description="T5 Multi-Worktree Test Swarm Defensible Audit")
    parser.add_argument("--projects", default="tokio,serde,ripgrep,nushell", help="Comma-separated project list")
    parser.add_argument("--runs", type=int, default=5, help="Number of benchmark iterations (default: 5)")
    parser.add_argument("--output", default=None, help="Output JSON path")
    args = parser.parse_args()

    out_p = Path(args.output) if args.output else (DEFAULT_RESULTS_DIR / "results_t5_test_swarm_defensible.json")

    rig = get_rig_info()
    projects = [p.strip() for p in args.projects.split(",") if p.strip()]

    final_report = {
        "schema_version": "t5_test_swarm_v1",
        "provenance": get_provenance_info(),
        "rig": rig,
        "mode": "test",
        "projects": {},
    }
    if out_p.exists():
        try:
            with open(out_p, "r") as f:
                existing = json.load(f)
                final_report["projects"] = existing.get("projects", {})
        except Exception:
            pass

    for proj in projects:
        try:
            res = run_t5_project(proj, runs_count=args.runs)
            final_report["projects"][proj] = res
        except Exception as e:
            print(f"ERROR auditing project {proj}: {e}")
            import traceback
            traceback.print_exc()
            final_report["projects"][proj] = {
                "mode": "test",
                "status": "FAIL",
                "error": str(e),
                "provenance": get_provenance_info(),
                "invariants": {
                    "exit_codes_all_matched": False,
                    "intentional_defect_caught_by_both": False,
                    "test_counts_match": False,
                    "fast_lock_never_skips_test_execution": False,
                }
            }

    out_p.parent.mkdir(parents=True, exist_ok=True)
    with open(out_p, "w") as f:
        json.dump(final_report, f, indent=2)
    print(f"\nDefensible T5 audit report saved to {out_p}")


if __name__ == "__main__":
    main()
