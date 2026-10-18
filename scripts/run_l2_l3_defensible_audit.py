#!/usr/bin/env python3
# Mitts Build — Rust-first build and package tooling with shared artifact cache
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0

"""
L2 & L3: Developer Lifecycle & Daily Workflow Defensible Audit
Measures multi-worktree FastBuildLock pure no-op (L2a), test planning no-op (L2b),
surgical single-leaf mutation (L3a), core crate leaf mutation (L3b), and
cross-worktree reuse of mutated artifacts with CAS immutability verification (L3c).
Includes parallel and sequential controls for both Mitts and Cargo.
"""

import argparse
import functools
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import threading
import time
from pathlib import Path

from benchmark_common import (
    get_binary_provenance,
    get_filesystem_type,
    get_rig_info,
)
from benchmark_worktree import (
    prepare_worktrees,
)

print = functools.partial(print, flush=True)

BENCH_ROOT = Path(os.environ.get("BENCH_ROOT", Path.home() / "mitts-benchmarks"))
DEFAULT_RESULTS_DIR = Path(
    os.environ.get("BENCHMARK_OUTPUT_DIR", BENCH_ROOT / "benchmark-results")
)
PROJECTS_DIR = BENCH_ROOT / "projects"
MITTS_BIN = Path(os.environ.get("MITTS_BIN", BENCH_ROOT / "mitts-bin"))


def check_same_device(path_a, path_b):
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
    for name in [
        "run_l2_l3_defensible_audit.py",
        "run_t5_defensible_audit.py",
        "benchmark_worktree.py",
        "benchmark_common.py",
    ]:
        p = scripts_dir / name
        if p.exists():
            script_shas[name] = hashlib.sha256(p.read_bytes()).hexdigest()

    combined = hashlib.sha256(
        "".join(f"{k}:{v}" for k, v in sorted(script_shas.items())).encode("utf-8")
    ).hexdigest()

    git_head = None
    scripts_dirty = None
    try:
        git_head = subprocess.check_output(
            ["git", "-C", str(scripts_dir), "rev-parse", "HEAD"],
            text=True,
            stderr=subprocess.DEVNULL,
        ).strip()
        status = subprocess.check_output(
            ["git", "-C", str(scripts_dir), "status", "--porcelain", "."],
            text=True,
            stderr=subprocess.DEVNULL,
        ).strip()
        scripts_dirty = bool(status)
    except Exception:
        pass

    return {
        "combined_sha256": combined,
        "scripts": script_shas,
        "git_commit": git_head,
        "scripts_dirty": scripts_dirty,
    }


def get_provenance_info(mitts_cache: Path, worktree_dir: Path) -> dict:
    prov = get_binary_provenance(MITTS_BIN)
    prov["scripts_provenance"] = get_scripts_provenance()
    prov["diagnostic_flags"] = {
        "mitts_no_reflink": os.environ.get("MITTS_NO_REFLINK", "0"),
        "mitts_no_hardlink": os.environ.get("MITTS_NO_HARDLINK", "0"),
        "mitts_cas_hardlink": os.environ.get("MITTS_CAS_HARDLINK", "0"),
        "mitts_timings": os.environ.get("MITTS_TIMINGS", "1"),
    }
    if mitts_cache:
        prov["cache_filesystem"] = get_filesystem_type(Path(mitts_cache))
    if worktree_dir:
        prov["worktree_filesystem"] = get_filesystem_type(Path(worktree_dir))
    if mitts_cache and worktree_dir:
        prov["same_device"] = check_same_device(mitts_cache, worktree_dir)
    return prov


def get_backing_disk_device(path: Path) -> str:
    try:
        res = subprocess.run(
            ["df", "-P", str(path)],
            capture_output=True,
            text=True,
            check=True,
        )
        lines = res.stdout.strip().split("\n")
        if len(lines) >= 2:
            dev_node = lines[1].split()[0]
            dev_name = Path(dev_node).name
            if dev_name.startswith("nvme") or dev_name.startswith("sd"):
                m = re.match(r"^([a-z0-9]+?)(?:p\d+|\d+)?$", dev_name)
                if m:
                    return m.group(1)
            return dev_name
    except Exception:
        pass
    return "nvme0n1"


def get_device_sectors_written(dev_name: str) -> int:
    stat_file = Path(f"/sys/class/block/{dev_name}/stat")
    if not stat_file.exists():
        for p in Path("/sys/class/block").glob(f"*{dev_name}*"):
            if (p / "stat").exists():
                stat_file = p / "stat"
                break
    if stat_file.exists():
        try:
            parts = stat_file.read_text().split()
            if len(parts) >= 7:
                return int(parts[6])
        except Exception:
            pass
    return 0


def wait_for_dirty_flush():
    try:
        subprocess.run(["sync"], check=True)
    except Exception:
        pass

    start = time.time()
    while time.time() - start < 5.0:
        dirty_kb = 0
        try:
            with open("/proc/meminfo", "r") as f:
                for line in f:
                    if line.startswith("Dirty:"):
                        dirty_kb = int(line.split()[1])
                        break
        except Exception:
            break
        if dirty_kb < 1024:
            break
        time.sleep(0.05)


def sanitize_cmd(cmd_list, bench_root: Path):
    res = []
    root_str = str(bench_root)
    home_str = str(Path.home())
    for item in cmd_list:
        s = str(item)
        if s.endswith("/mitts-bin") or s == "mitts-bin":
            res.append("mitts-bin")
        elif s.startswith(root_str):
            res.append("$BENCH_ROOT" + s[len(root_str):])
        elif s.startswith(home_str):
            res.append("~" + s[len(home_str):])
        else:
            res.append(s)
    return res


def sanitize_stderr(text: str, bench_root: Path) -> str:
    root_str = str(bench_root)
    home_str = str(Path.home())
    res = text.replace(root_str, "$BENCH_ROOT")
    res = res.replace(home_str, "~")
    return res


def parse_mitts_stats(text: str) -> dict:
    res = {
        "compiled": 0,
        "cached": 0,
        "total": 0,
        "fast_lock": 0,
        "fast_lock_verdict": None,
    }
    m = re.search(
        r"\[mitts::stats\]\s+compiled=(\d+)\s+cached=(\d+)\s+(?:tainted=\d+\s+)?total=(\d+)(?:\s+fast_lock=(\d+))?",
        text,
    )
    if m:
        res["compiled"] = int(m.group(1))
        res["cached"] = int(m.group(2))
        res["total"] = int(m.group(3))
        if m.group(4) is not None:
            res["fast_lock"] = int(m.group(4))
    if "fast-lock hit" in text or "fast_lock=1" in text:
        res["fast_lock_verdict"] = "HIT"
    elif "fast-lock miss" in text:
        res["fast_lock_verdict"] = "MISS"
    return res


def cargo_compiling_packages(stderr: str) -> int:
    return len(re.findall(r"^\s*Compiling\s+", stderr, re.MULTILINE))


def compute_quantiles(values):
    if not values:
        return {"min": 0, "max": 0, "p50": 0}
    s = sorted(values)
    n = len(s)
    p50_idx = int(round(0.50 * (n - 1)))
    return {
        "min": round(s[0], 4),
        "max": round(s[-1], 4),
        "p50": round(s[p50_idx], 4),
    }


def calibrate_python_popen_baseline(count=12):
    t0 = time.time()
    procs = [
        subprocess.Popen(
            ["/bin/true"],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        for _ in range(count)
    ]
    for p in procs:
        p.wait()
    wall = time.time() - t0
    return round(wall * 1000.0, 2)


def run_concurrent_worktree_cmd(cmd_fn, worktrees, env=None, bench_root=BENCH_ROOT):
    results = [None] * len(worktrees)
    t0 = time.time()

    def run_one(idx, wt_dir):
        cmd = cmd_fn(wt_dir)
        proc_env = os.environ.copy()
        if env:
            proc_env.update(env)
        start = time.time()
        p = subprocess.Popen(
            cmd,
            cwd=str(wt_dir),
            env=proc_env,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        stdout, stderr = p.communicate()
        elapsed = time.time() - start
        stats = parse_mitts_stats(stdout + "\n" + stderr)
        results[idx] = {
            "wt_index": idx + 1,
            "wall_sec": round(elapsed, 4),
            "exit_code": p.returncode,
            "mitts_stats": stats,
            "cargo_compiled_packages": cargo_compiling_packages(stderr),
            "stdout": stdout,
            "stderr": sanitize_stderr(stderr, bench_root),
            "cmd": sanitize_cmd(cmd, bench_root),
        }

    threads = []
    for i, (_, wt_dir, _) in enumerate(worktrees):
        th = threading.Thread(target=run_one, args=(i, wt_dir))
        threads.append(th)
        th.start()

    for th in threads:
        th.join()

    total_wall = time.time() - t0
    per_wt_times = [r["wall_sec"] for r in results]
    quantiles = compute_quantiles(per_wt_times)

    return {
        "total_wall_sec": round(total_wall, 4),
        "quantiles": quantiles,
        "results": results,
    }


def run_sequential_worktree_cmd(cmd_fn, worktrees, env=None, bench_root=BENCH_ROOT):
    results = []
    t0 = time.time()

    for idx, (_, wt_dir, _) in enumerate(worktrees):
        cmd = cmd_fn(wt_dir)
        proc_env = os.environ.copy()
        if env:
            proc_env.update(env)
        start = time.time()
        p = subprocess.run(
            cmd,
            cwd=str(wt_dir),
            env=proc_env,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        elapsed = time.time() - start
        stats = parse_mitts_stats(p.stdout + "\n" + p.stderr)
        results.append({
            "wt_index": idx + 1,
            "wall_sec": round(elapsed, 4),
            "exit_code": p.returncode,
            "mitts_stats": stats,
            "cargo_compiled_packages": cargo_compiling_packages(p.stderr),
            "stdout": p.stdout,
            "stderr": sanitize_stderr(p.stderr, bench_root),
            "cmd": sanitize_cmd(cmd, bench_root),
        })

    total_wall = time.time() - t0
    per_wt_times = [r["wall_sec"] for r in results]
    quantiles = compute_quantiles(per_wt_times)

    return {
        "total_wall_sec": round(total_wall, 4),
        "quantiles": quantiles,
        "results": results,
    }


def snapshot_cas_objects(cas_dir: Path):
    objects_dir = cas_dir / "cas" / "objects"
    if not objects_dir.exists():
        return {}
    snap = {}
    for p in objects_dir.glob("*"):
        if p.is_file():
            try:
                st = p.stat()
                snap[p.name] = (st.st_size, st.st_mtime_ns)
            except Exception:
                pass
    return snap


def verify_cas_immutability(before_snap: dict, cas_dir: Path) -> dict:
    after_snap = snapshot_cas_objects(cas_dir)
    mutated = []
    removed = []
    added = []

    for name, before_meta in before_snap.items():
        if name not in after_snap:
            removed.append(name)
        elif after_snap[name] != before_meta:
            mutated.append({
                "name": name,
                "before": before_meta,
                "after": after_snap[name],
            })

    for name in after_snap:
        if name not in before_snap:
            added.append(name)

    return {
        "immutable": len(mutated) == 0 and len(removed) == 0,
        "mutated_count": len(mutated),
        "removed_count": len(removed),
        "new_objects_added": len(added),
        "mutated_objects": mutated,
    }


def main():
    parser = argparse.ArgumentParser(
        description="Mitts Defensible L2 & L3 Benchmarks"
    )
    parser.add_argument(
        "--project",
        default="tokio",
        choices=["tokio", "ripgrep", "serde"],
        help="Target benchmark repository",
    )
    parser.add_argument(
        "--worktrees",
        type=int,
        default=12,
        help="Number of concurrent git worktrees",
    )
    parser.add_argument(
        "--iterations",
        type=int,
        default=1,
        help="Number of benchmark iterations",
    )
    args = parser.parse_args()

    project_dir = PROJECTS_DIR / args.project
    if not project_dir.exists():
        print(f"ERROR: Project directory not found: {project_dir}")
        sys.exit(1)

    scratch_dir = BENCH_ROOT / f"scratch_l2_l3_{args.project}"
    wt_base = scratch_dir / "worktrees"
    mitts_cache = scratch_dir / "cache"

    scratch_dir.mkdir(parents=True, exist_ok=True)
    mitts_cache.mkdir(parents=True, exist_ok=True)

    try:
        head_commit = (
            subprocess.check_output(
                ["git", "rev-parse", "HEAD"], cwd=str(project_dir)
            )
            .decode()
            .strip()
        )
    except Exception as e:
        print(f"ERROR: Failed to get git commit for {project_dir}: {e}")
        sys.exit(1)

    commits = [head_commit] * args.worktrees
    print(f"Preparing {args.worktrees} identical worktrees from {head_commit[:8]}...")
    wts = prepare_worktrees(project_dir, wt_base, commits)

    dev_name = get_backing_disk_device(wt_base)
    prov = get_provenance_info(mitts_cache=mitts_cache, worktree_dir=wt_base)

    print("\n=======================================================")
    print(f"  Starting L2 & L3 Defensible Audit: {args.project}")
    print(f"  Rig: {get_rig_info().get('cpu_model', 'CPU')} | Backing dev: {dev_name}")
    print(f"  Mitts SHA: {prov.get('binary_sha256', '')[:16]} | Git: {prov.get('git_commit', '')}")
    print("=======================================================")

    popen_baseline_ms = calibrate_python_popen_baseline(args.worktrees)
    print(f"Baseline Python Popen 12x /bin/true: {popen_baseline_ms:.2f} ms")

    mitts_env = {
        "MITTS_CACHE_DIR": str(mitts_cache),
        "MITTS_TIMINGS": "1",
    }
    if os.environ.get("MITTS_CAS_HARDLINK") == "1":
        mitts_env["MITTS_CAS_HARDLINK"] = "1"

    # [Step 1/5] Warm up all 12 worktrees with mitts build & cargo build
    print("\n[1/5] Warming up Mitts build across all 12 worktrees...")
    w0_cmd = lambda d: [str(MITTS_BIN), "build"]
    res_w0 = run_concurrent_worktree_cmd(w0_cmd, wts, env=mitts_env)
    print(f"  Initial Mitts warm-up wall time: {res_w0['total_wall_sec']:.2f} s")
    for r in res_w0["results"]:
        if r["exit_code"] != 0:
            print(f"ERROR: Warm-up failed in WT-{r['wt_index']:02d}: {r['stderr']}")
            sys.exit(1)

    # [Step 2/5] Benchmark Vector L2a: Pure FastBuildLock No-Op
    print("\n[2/5] Benchmarking Vector L2a: Pure FastBuildLock No-Op (37 compilation units)...")
    l2a_mitts_runs = []
    l2a_cargo_parallel_runs = []
    l2a_cargo_sequential_runs = []

    for it in range(1, args.iterations + 1):
        wait_for_dirty_flush()
        # Mitts L2a (parallel across 12 worktrees)
        sec_before = get_device_sectors_written(dev_name)
        res_m = run_concurrent_worktree_cmd(
            lambda d: [str(MITTS_BIN), "build"], wts, env=mitts_env
        )
        sec_after = get_device_sectors_written(dev_name)
        dev_wr_mb = round((sec_after - sec_before) * 512 / (1024.0 * 1024.0), 2)
        res_m["device_mb_written"] = dev_wr_mb

        for r in res_m["results"]:
            if r["exit_code"] != 0 or r["mitts_stats"].get("fast_lock") != 1:
                print(f"ERROR: Mitts L2a invariant failed in WT-{r['wt_index']:02d}: exit={r['exit_code']}, fast_lock={r['mitts_stats'].get('fast_lock')}")
                r["invalid_run"] = True
        l2a_mitts_runs.append(res_m)

        # Warm cargo on all 12 WTs once before cargo no-op benchmark
        if it == 1:
            run_concurrent_worktree_cmd(
                lambda d: ["cargo", "build", "--offline", "--workspace"], wts
            )

        wait_for_dirty_flush()
        # Cargo L2a (parallel across 12 worktrees - exhibits package cache lock contention)
        sec_before = get_device_sectors_written(dev_name)
        res_c_par = run_concurrent_worktree_cmd(
            lambda d: ["cargo", "build", "--offline", "--workspace"], wts
        )
        sec_after = get_device_sectors_written(dev_name)
        dev_wr_mb = round((sec_after - sec_before) * 512 / (1024.0 * 1024.0), 2)
        res_c_par["device_mb_written"] = dev_wr_mb
        for r in res_c_par["results"]:
            if r["exit_code"] != 0 or "Finished" not in r["stderr"]:
                print(f"ERROR: Cargo parallel L2a failed in WT-{r['wt_index']:02d}: exit={r['exit_code']}")
                r["invalid_run"] = True
        l2a_cargo_parallel_runs.append(res_c_par)

        wait_for_dirty_flush()
        # Cargo L2a (sequential control - 12 worktrees run sequentially without lock contention)
        sec_before = get_device_sectors_written(dev_name)
        res_c_seq = run_sequential_worktree_cmd(
            lambda d: ["cargo", "build", "--offline", "--workspace"], wts
        )
        sec_after = get_device_sectors_written(dev_name)
        dev_wr_mb = round((sec_after - sec_before) * 512 / (1024.0 * 1024.0), 2)
        res_c_seq["device_mb_written"] = dev_wr_mb
        for r in res_c_seq["results"]:
            if r["exit_code"] != 0 or "Finished" not in r["stderr"]:
                print(f"ERROR: Cargo sequential L2a failed in WT-{r['wt_index']:02d}: exit={r['exit_code']}")
                r["invalid_run"] = True
        l2a_cargo_sequential_runs.append(res_c_seq)

        mq = res_m["quantiles"]
        cpq = res_c_par["quantiles"]
        csq = res_c_seq["quantiles"]
        print(f"  [Iter {it}] Mitts Parallel No-Op (fast-lock) : wall={res_m['total_wall_sec']:.3f}s | p50={mq['p50']*1000:.1f}ms, max={mq['max']*1000:.1f}ms | dev_wr={res_m['device_mb_written']}MB")
        print(f"  [Iter {it}] Cargo Parallel No-Op (contention) : wall={res_c_par['total_wall_sec']:.3f}s | p50={cpq['p50']*1000:.1f}ms, max={cpq['max']*1000:.1f}ms | dev_wr={res_c_par['device_mb_written']}MB")
        print(f"  [Iter {it}] Cargo Sequential No-Op (control)   : wall={res_c_seq['total_wall_sec']:.3f}s | p50={csq['p50']*1000:.1f}ms, max={csq['max']*1000:.1f}ms | dev_wr={res_c_seq['device_mb_written']}MB")

    # [Step 3/5] Benchmark Vector L2b: Test No-Op Planning (test --tests --no-run --workspace)
    print("\n[3/5] Benchmarking Vector L2b: Test No-Op Planning (343 units)...")
    # Warm tests on all 12 worktrees first
    run_concurrent_worktree_cmd(
        lambda d: [str(MITTS_BIN), "test", "--tests", "--no-run", "--workspace"],
        wts,
        env=mitts_env,
    )
    run_concurrent_worktree_cmd(
        lambda d: ["cargo", "test", "--offline", "--tests", "--no-run", "--workspace"], wts
    )

    wait_for_dirty_flush()
    # Mitts Parallel
    res_m_test_par = run_concurrent_worktree_cmd(
        lambda d: [str(MITTS_BIN), "test", "--tests", "--no-run", "--workspace"],
        wts,
        env=mitts_env,
    )

    wait_for_dirty_flush()
    # Mitts Sequential Control
    res_m_test_seq = run_sequential_worktree_cmd(
        lambda d: [str(MITTS_BIN), "test", "--tests", "--no-run", "--workspace"],
        wts,
        env=mitts_env,
    )

    wait_for_dirty_flush()
    # Cargo Parallel
    res_c_test_par = run_concurrent_worktree_cmd(
        lambda d: ["cargo", "test", "--offline", "--tests", "--no-run", "--workspace"], wts
    )

    wait_for_dirty_flush()
    # Cargo Sequential Control
    res_c_test_seq = run_sequential_worktree_cmd(
        lambda d: ["cargo", "test", "--offline", "--tests", "--no-run", "--workspace"], wts
    )

    mp_tq = res_m_test_par["quantiles"]
    ms_tq = res_m_test_seq["quantiles"]
    cp_tq = res_c_test_par["quantiles"]
    cs_tq = res_c_test_seq["quantiles"]
    print(f"  Mitts Parallel test --no-run   : wall={res_m_test_par['total_wall_sec']:.3f}s | p50={mp_tq['p50']*1000:.1f}ms, max={mp_tq['max']*1000:.1f}ms")
    print(f"  Mitts Sequential test --no-run : wall={res_m_test_seq['total_wall_sec']:.3f}s | p50={ms_tq['p50']*1000:.1f}ms, max={ms_tq['max']*1000:.1f}ms")
    print(f"  Cargo Parallel test --no-run   : wall={res_c_test_par['total_wall_sec']:.3f}s | p50={cp_tq['p50']*1000:.1f}ms, max={cp_tq['max']*1000:.1f}ms")
    print(f"  Cargo Sequential test --no-run : wall={res_c_test_seq['total_wall_sec']:.3f}s | p50={cs_tq['p50']*1000:.1f}ms, max={cs_tq['max']*1000:.1f}ms")

    # [Step 4/5] Benchmark Vector L3a: Surgical Leaf Mutation (tests/sync_mpsc.rs in WT-01)
    print("\n[4/5] Benchmarking Vector L3a: Surgical Leaf Mutation in WT-01...")
    wt01_dir = wts[0][1]
    mpsc_test_file = wt01_dir / "tokio" / "tests" / "sync_mpsc.rs"
    if not mpsc_test_file.exists():
        candidates = list(wt01_dir.glob("**/tests/*.rs"))
        if candidates:
            mpsc_test_file = candidates[0]

    import uuid
    run_uid = uuid.uuid4().hex[:6]

    original_test_content = mpsc_test_file.read_text()
    mutation_probe = f"\n\n#[test]\nfn __mitts_l3a_probe_test_{run_uid}() {{\n    assert_eq!(\"probe_l3a\", \"probe_l3a\");\n}}\n"

    try:
        mpsc_test_file.write_text(original_test_content + mutation_probe)
        wait_for_dirty_flush()

        res_l3a = run_concurrent_worktree_cmd(
            lambda d: [str(MITTS_BIN), "test", "--tests", "--no-run", "--workspace"],
            wts,
            env=mitts_env,
        )

        wt01_stats = res_l3a["results"][0]["mitts_stats"]
        wt_other_stats = [r["mitts_stats"] for r in res_l3a["results"][1:]]

        print(f"  Mitts WT-01 (mutated test file): compiled={wt01_stats.get('compiled')} | cached={wt01_stats.get('cached')} | wall={res_l3a['results'][0]['wall_sec']:.3f}s")
        wt_other_compiled = sum(s.get("compiled", 0) for s in wt_other_stats)
        wt_other_cached = sum(s.get("cached", 0) for s in wt_other_stats)
        print(f"  Mitts WT-02..12 (untouched): total_compiled={wt_other_compiled} | total_cached={wt_other_cached}")

        l3a_invariant_passed = (wt01_stats.get("compiled", 0) == 1) and (wt_other_compiled == 0)
        print(f"  Invariant L3a (WT-01 compiled==1, WT-02..12 compiled==0): {l3a_invariant_passed}")

        # Measured Cargo baseline on same mutated leaf in WT-01
        wait_for_dirty_flush()
        start_c = time.time()
        p_c = subprocess.run(
            ["cargo", "test", "--offline", "--tests", "--no-run", "--workspace"],
            cwd=str(wt01_dir),
            capture_output=True,
            text=True,
        )
        wall_c = time.time() - start_c
        cargo_compiled_l3a = cargo_compiling_packages(p_c.stderr)
        print(f"  Cargo WT-01 (mutated test file): packages={cargo_compiled_l3a} | wall={wall_c:.3f}s (exit {p_c.returncode})")

        cargo_l3a_result = {
            "cmd": ["cargo", "test", "--offline", "--tests", "--no-run", "--workspace"],
            "exit_code": p_c.returncode,
            "wall_sec": round(wall_c, 4),
            "cargo_compiled_packages": cargo_compiled_l3a,
            "stdout": p_c.stdout,
            "stderr": sanitize_stderr(p_c.stderr, BENCH_ROOT),
        }
    finally:
        mpsc_test_file.write_text(original_test_content)

    # [Step 5/5] Benchmark Vector L3b & L3c: Core Module Mutation & Cross-WT Reuse
    print("\n[5/5] Benchmarking Vector L3b & L3c: Core Module Mutation & Cross-Worktree Reuse...")
    mpsc_mod_file = wt01_dir / "tokio" / "src" / "sync" / "mpsc" / "mod.rs"
    if not mpsc_mod_file.exists():
        candidates = list(wt01_dir.glob("**/src/**/*.rs"))
        if candidates:
            mpsc_mod_file = candidates[0]

    original_mod_content = mpsc_mod_file.read_text()
    mod_probe = f"\n\n#[doc(hidden)]\npub fn __mitts_l3b_probe_func_{run_uid}() -> usize {{\n    42\n}}\n"

    cas_before = snapshot_cas_objects(mitts_cache)
    print(f"  CAS snapshot before L3b: {len(cas_before)} objects tracked")

    try:
        # Mutate WT-01
        mpsc_mod_file.write_text(original_mod_content + mod_probe)
        wait_for_dirty_flush()

        res_l3b = run_concurrent_worktree_cmd(
            lambda d: [str(MITTS_BIN), "test", "--tests", "--no-run", "--workspace"],
            wts,
            env=mitts_env,
        )

        wt01_l3b = res_l3b["results"][0]["mitts_stats"]
        wt_other_l3b_compiled = sum(
            r["mitts_stats"].get("compiled", 0) for r in res_l3b["results"][1:]
        )
        wt_other_l3b_cached = sum(
            r["mitts_stats"].get("cached", 0) for r in res_l3b["results"][1:]
        )

        print(f"  Mitts WT-01 (mutated core crate): compiled={wt01_l3b.get('compiled')} | cached={wt01_l3b.get('cached')} | wall={res_l3b['results'][0]['wall_sec']:.3f}s")
        print(f"  Mitts WT-02..12 (untouched): total_compiled={wt_other_l3b_compiled} | total_cached={wt_other_l3b_cached}")

        cas_audit_l3b = verify_cas_immutability(cas_before, mitts_cache)
        print(f"  CAS Immutability Audit L3b: immutable={cas_audit_l3b['immutable']} | mutated={cas_audit_l3b['mutated_count']} | added={cas_audit_l3b['new_objects_added']}")

        # Measured Cargo baseline on core module mutation in WT-01
        wait_for_dirty_flush()
        start_cb = time.time()
        p_cb = subprocess.run(
            ["cargo", "test", "--offline", "--tests", "--no-run", "--workspace"],
            cwd=str(wt01_dir),
            capture_output=True,
            text=True,
        )
        wall_cb = time.time() - start_cb
        cargo_compiled_l3b = cargo_compiling_packages(p_cb.stderr)
        print(f"  Cargo WT-01 (mutated core crate): packages={cargo_compiled_l3b} | wall={wall_cb:.3f}s (exit {p_cb.returncode})")

        cargo_l3b_result = {
            "cmd": ["cargo", "test", "--offline", "--tests", "--no-run", "--workspace"],
            "exit_code": p_cb.returncode,
            "wall_sec": round(wall_cb, 4),
            "cargo_compiled_packages": cargo_compiled_l3b,
            "stdout": p_cb.stdout,
            "stderr": sanitize_stderr(p_cb.stderr, BENCH_ROOT),
        }

        # Measured Cargo baseline with CARGO_INCREMENTAL=0 on identical mutation in WT-03
        wt03_dir = wts[2][1]
        wt03_mpsc_mod = wt03_dir / "tokio" / "src" / "sync" / "mpsc" / "mod.rs"
        if not wt03_mpsc_mod.exists():
            candidates = list(wt03_dir.glob("**/src/**/*.rs"))
            if candidates:
                wt03_mpsc_mod = candidates[0]

        wt03_orig_content = wt03_mpsc_mod.read_text()
        try:
            wt03_mpsc_mod.write_text(wt03_orig_content + mod_probe)
            wait_for_dirty_flush()

            cargo_noinc_env = os.environ.copy()
            cargo_noinc_env["CARGO_INCREMENTAL"] = "0"

            start_noinc = time.time()
            p_noinc = subprocess.run(
                ["cargo", "test", "--offline", "--tests", "--no-run", "--workspace"],
                cwd=str(wt03_dir),
                env=cargo_noinc_env,
                capture_output=True,
                text=True,
            )
            wall_noinc = time.time() - start_noinc
            cargo_compiled_noinc = cargo_compiling_packages(p_noinc.stderr)
            print(f"  Cargo WT-03 (CARGO_INCREMENTAL=0 control): packages={cargo_compiled_noinc} | wall={wall_noinc:.3f}s (exit {p_noinc.returncode})")

            cargo_l3b_noinc_result = {
                "cmd": ["cargo", "test", "--offline", "--tests", "--no-run", "--workspace"],
                "env_overrides": {"CARGO_INCREMENTAL": "0"},
                "exit_code": p_noinc.returncode,
                "wall_sec": round(wall_noinc, 4),
                "cargo_compiled_packages": cargo_compiled_noinc,
                "stdout": p_noinc.stdout,
                "stderr": sanitize_stderr(p_noinc.stderr, BENCH_ROOT),
            }
        finally:
            wt03_mpsc_mod.write_text(wt03_orig_content)

        # L3c: Apply the exact same mutation in WT-02 and verify cache hit from WT-01's CAS
        print("\n  [L3c] Applying same mutation to WT-02...")
        wt02_dir = wts[1][1]
        wt02_mpsc_mod = wt02_dir / "tokio" / "src" / "sync" / "mpsc" / "mod.rs"
        if not wt02_mpsc_mod.exists():
            candidates = list(wt02_dir.glob("**/src/**/*.rs"))
            if candidates:
                wt02_mpsc_mod = candidates[0]

        wt02_orig_content = wt02_mpsc_mod.read_text()
        cas_before_l3c = snapshot_cas_objects(mitts_cache)
        try:
            wt02_mpsc_mod.write_text(wt02_orig_content + mod_probe)
            wait_for_dirty_flush()

            cmd_wt02 = [str(MITTS_BIN), "test", "--tests", "--no-run", "--workspace"]
            proc_env = os.environ.copy()
            proc_env.update(mitts_env)

            start_wt02 = time.time()
            p = subprocess.run(
                cmd_wt02,
                cwd=str(wt02_dir),
                env=proc_env,
                capture_output=True,
                text=True,
            )
            elapsed_wt02 = time.time() - start_wt02
            wt02_stats = parse_mitts_stats(p.stdout + "\n" + p.stderr)

            raw_wt02_result = {
                "cmd": sanitize_cmd(cmd_wt02, BENCH_ROOT),
                "exit_code": p.returncode,
                "wall_sec": round(elapsed_wt02, 4),
                "stdout": p.stdout,
                "stderr": sanitize_stderr(p.stderr, BENCH_ROOT),
                "mitts_stats": wt02_stats,
            }

            print(f"  WT-02 (reusing WT-01's CAS artifact): compiled={wt02_stats.get('compiled')} | cached={wt02_stats.get('cached')} | wall={elapsed_wt02:.3f}s (exit {p.returncode})")
            l3c_hit = (
                p.returncode == 0
                and wt02_stats.get("compiled", 0) == 0
                and wt02_stats.get("cached", 0) > 0
            )
            print(f"  Invariant L3c (WT-02 cache hit from WT-01 CAS): {l3c_hit}")

            cas_audit_l3c = verify_cas_immutability(cas_before_l3c, mitts_cache)
            print(f"  CAS Immutability Audit L3c: immutable={cas_audit_l3c['immutable']} | mutated={cas_audit_l3c['mutated_count']} | added={cas_audit_l3c['new_objects_added']}")

            # Verify WT-03..12 remain untouched with 0 compilations
            res_other_after_wt02 = run_concurrent_worktree_cmd(
                lambda d: [str(MITTS_BIN), "test", "--tests", "--no-run", "--workspace"],
                wts[2:],
                env=mitts_env,
            )
            other_after_comp = sum(
                r["mitts_stats"].get("compiled", 0) for r in res_other_after_wt02["results"]
            )
            print(f"  WT-03..12 untouched verification: compiled={other_after_comp} (expected 0)")
        finally:
            wt02_mpsc_mod.write_text(wt02_orig_content)

    finally:
        mpsc_mod_file.write_text(original_mod_content)

    # Format JSON report
    report = {
        "schema_version": "mitts_l2_l3_defensible_v3",
        "provenance": prov,
        "rig": get_rig_info(),
        "baseline_python_popen_12x_ms": popen_baseline_ms,
        "l2a_fast_build_lock_no_op": {
            "units_checked": 37,
            "iterations": args.iterations,
            "mitts_runs": l2a_mitts_runs,
            "cargo_parallel_runs": l2a_cargo_parallel_runs,
            "cargo_sequential_runs": l2a_cargo_sequential_runs,
            "speedup_vs_cargo_parallel_p50": round(
                l2a_cargo_parallel_runs[-1]["quantiles"]["p50"]
                / max(l2a_mitts_runs[-1]["quantiles"]["p50"], 0.0001),
                2,
            ),
            "speedup_vs_cargo_sequential_p50": round(
                l2a_cargo_sequential_runs[-1]["quantiles"]["p50"]
                / max(l2a_mitts_runs[-1]["quantiles"]["p50"], 0.0001),
                2,
            ),
        },
        "l2b_test_no_op_planning": {
            "units_checked": 343,
            "mitts_parallel": res_m_test_par,
            "mitts_sequential": res_m_test_seq,
            "cargo_parallel": res_c_test_par,
            "cargo_sequential": res_c_test_seq,
        },
        "l3a_surgical_leaf_mutation": {
            "mitts": {
                "wt01_stats": wt01_stats,
                "wt_other_compiled": wt_other_compiled,
                "wt_other_cached": wt_other_cached,
                "invariant_passed": l3a_invariant_passed,
                "raw_results": res_l3a["results"],
            },
            "cargo": cargo_l3a_result,
        },
        "l3b_core_crate_mutation": {
            "mitts": {
                "wt01_stats": wt01_l3b,
                "wt_other_compiled": wt_other_l3b_compiled,
                "wt_other_cached": wt_other_l3b_cached,
                "cas_immutability_audit": cas_audit_l3b,
                "raw_results": res_l3b["results"],
            },
            "cargo": cargo_l3b_result,
            "cargo_noincremental": cargo_l3b_noinc_result,
        },
        "l3c_cross_worktree_reuse": {
            "wt02_raw_result": raw_wt02_result,
            "hit_from_wt01_cas": l3c_hit,
            "cas_immutability_audit": cas_audit_l3c,
            "wt03_12_compiled": other_after_comp,
            "wt03_12_raw_results": res_other_after_wt02["results"],
        },
        "invariants": {
            "l2a_pure_noop_subsecond": all(
                r["exit_code"] == 0 and r["mitts_stats"].get("fast_lock") == 1 and r["wall_sec"] < 1.0
                for r in l2a_mitts_runs[-1]["results"]
            ),
            "l2b_isolated_planning_fast": (
                res_m_test_par.get("total_wall_sec", 999.0) < 1.0
                and all(r["exit_code"] == 0 for r in res_m_test_par["results"])
            ),
            "l3a_leaf_compiles_single_unit": l3a_invariant_passed,
            "l3b_sibling_wt_recompiles_zero": (wt_other_l3b_compiled == 0),
            "l3c_cas_zero_mutation_and_immutable": (
                cas_audit_l3c.get("immutable", False)
                and cas_audit_l3c.get("mutated_count", -1) == 0
                and cas_audit_l3c.get("new_objects_added", -1) == 0
                and l3c_hit
            ),
        },
    }

    out_file = DEFAULT_RESULTS_DIR / "results_l2_l3_defensible_audit.json"
    out_file.parent.mkdir(parents=True, exist_ok=True)
    out_file.write_text(json.dumps(report, indent=2))
    print(f"\nDefensible L2/L3 audit report saved to {out_file}")


if __name__ == "__main__":
    main()
