#!/usr/bin/env python3
import concurrent.futures
import json
import os
import shutil
import subprocess
import sys
import time
from pathlib import Path

MITTS_BIN = Path("/home/user/mitts/target/release/mitts")
PROJECTS_DIR = Path("/home/user/mitts-benchmarks/projects")
BENCH_ROOT = Path("/home/user/mitts-benchmarks")

def measure_command(cmd, cwd, env=None):
    start = time.perf_counter()
    p = subprocess.Popen(
        cmd,
        cwd=cwd,
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True
    )
    # wait4 captures child and all grandchild processes (rustc etc)
    _, status, ru = os.wait4(p.pid, 0)
    stdout, stderr = p.communicate()
    elapsed = time.perf_counter() - start
    
    exit_code = os.waitstatus_to_exitcode(status) if hasattr(os, "waitstatus_to_exitcode") else (status >> 8)
    utime = ru.ru_utime
    stime = ru.ru_stime
    total_cpu = utime + stime
    cpu_percent = (total_cpu / max(elapsed, 0.001)) * 100.0
    max_rss_mb = ru.ru_maxrss / 1024.0 # Linux ru_maxrss is in KB
    
    return {
        "wall_sec": round(elapsed, 3),
        "user_sec": round(utime, 3),
        "sys_sec": round(stime, 3),
        "total_cpu_sec": round(total_cpu, 3),
        "cpu_percent": round(cpu_percent, 1),
        "max_rss_mb": round(max_rss_mb, 1),
        "exit_code": exit_code,
        "stdout_tail": stdout[-500:] if stdout else "",
        "stderr_tail": stderr[-500:] if stderr else ""
    }

def get_dir_size_mb(path):
    p = Path(path)
    if not p.exists():
        return 0.0
    total = 0
    for root, dirs, files in os.walk(p):
        for f in files:
            fp = os.path.join(root, f)
            if not os.path.islink(fp):
                try:
                    total += os.path.getsize(fp)
                except OSError:
                    pass
    return round(total / (1024.0 * 1024.0), 2)

def prepare_worktrees(project_name, repo_dir, worktree_base_dir, num_worktrees=30):
    worktree_base_dir.mkdir(parents=True, exist_ok=True)
    worktrees = []
    
    # Check if worktrees already exist
    existing_wts = sorted([d for d in worktree_base_dir.glob("wt-*") if d.is_dir()])
    if len(existing_wts) >= num_worktrees:
        print(f"[{project_name}] Using existing {num_worktrees} worktrees in {worktree_base_dir}")
        for i, wt_dir in enumerate(existing_wts[:num_worktrees], start=1):
            worktrees.append((i, wt_dir))
        return worktrees

    print(f"[{project_name}] Creating {num_worktrees} git worktrees from {repo_dir}...")
    for i in range(1, num_worktrees + 1):
        wt_dir = worktree_base_dir / f"wt-{i:02d}"
        if not wt_dir.exists():
            cmd = ["git", "worktree", "add", "-d", str(wt_dir), "HEAD"]
            subprocess.run(cmd, cwd=repo_dir, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        lock_file = wt_dir / "Cargo.lock"
        if not lock_file.exists():
            subprocess.run(["cargo", "generate-lockfile"], cwd=wt_dir, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        worktrees.append((i, wt_dir))
    print(f"[{project_name}] {num_worktrees} worktrees ready.")
    return worktrees

def run_parallel_builds(command_builder_fn, worktrees, max_workers=30):
    start = time.perf_counter()
    results = []
    with concurrent.futures.ThreadPoolExecutor(max_workers=max_workers) as executor:
        futures = {executor.submit(command_builder_fn, wt): wt for wt in worktrees}
        for fut in concurrent.futures.as_completed(futures):
            wt = futures[fut]
            try:
                res = fut.result()
                results.append((wt, res))
            except Exception as e:
                results.append((wt, {
                    "wall_sec": -1, "user_sec": 0, "sys_sec": 0, "total_cpu_sec": 0,
                    "cpu_percent": 0, "max_rss_mb": 0, "exit_code": -1, "stderr_tail": str(e)
                }))
    total_wall = time.perf_counter() - start
    
    total_user = sum(r["user_sec"] for _, r in results if r["exit_code"] == 0)
    total_sys = sum(r["sys_sec"] for _, r in results if r["exit_code"] == 0)
    total_cpu = total_user + total_sys
    avg_cpu_percent = (total_cpu / max(total_wall, 0.001)) * 100.0
    peak_rss = max([r["max_rss_mb"] for _, r in results] + [0.0])
    passes = sum(1 for _, r in results if r["exit_code"] == 0)

    return {
        "wall_sec": round(total_wall, 3),
        "user_sec": round(total_user, 3),
        "sys_sec": round(total_sys, 3),
        "total_cpu_sec": round(total_cpu, 3),
        "cpu_percent": round(avg_cpu_percent, 1),
        "max_rss_mb": round(peak_rss, 1),
        "success_count": passes,
        "total_count": len(worktrees)
    }

def benchmark_project(project_name, project_repo_dir, worktree_base_dir):
    print(f"\n=======================================================")
    print(f"🚀 BENCHMARKING PROJECT: {project_name.upper()}")
    print(f"=======================================================")
    results = {}
    
    single_wt = project_repo_dir
    shared_cas = BENCH_ROOT / f"cas_bench_{project_name}"
    
    # -------------------------------------------------------------
    # 1. Single-Worktree Cold Start (Mitts Cold vs Cargo Cold)
    # -------------------------------------------------------------
    print(f"\n--- [1] Single-Worktree Cold Build ---")
    
    # 1A. Cargo Pure Cold
    subprocess.run(["cargo", "clean"], cwd=single_wt, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    cargo_env = os.environ.copy()
    cargo_env["RUSTC_WRAPPER"] = "" # pure cold cargo without sccache
    print(f"Running Cargo Single Cold...")
    cargo_cold = measure_command(["cargo", "build"], single_wt, env=cargo_env)
    cargo_cold_disk = get_dir_size_mb(single_wt / "target")
    cargo_cold["disk_written_mb"] = cargo_cold_disk
    print(f"  -> Cargo Cold: {cargo_cold['wall_sec']}s | CPU: {cargo_cold['cpu_percent']}% | Disk: {cargo_cold_disk}MB")

    # 1B. Mitts Pure Cold
    if shared_cas.exists():
        shutil.rmtree(shared_cas)
    if (single_wt / ".mitts").exists():
        shutil.rmtree(single_wt / ".mitts")
    mitts_env = os.environ.copy()
    mitts_env["MITTS_CACHE_DIR"] = str(shared_cas)
    print(f"Running Mitts Single Cold...")
    mitts_cold = measure_command([str(MITTS_BIN), "build"], single_wt, env=mitts_env)
    mitts_cold_disk = get_dir_size_mb(single_wt / ".mitts") + get_dir_size_mb(shared_cas)
    mitts_cold["disk_written_mb"] = mitts_cold_disk
    print(f"  -> Mitts Cold: {mitts_cold['wall_sec']}s | CPU: {mitts_cold['cpu_percent']}% | Disk: {mitts_cold_disk}MB")
    
    results["single_cold"] = {
        "cargo": cargo_cold,
        "mitts": mitts_cold,
        "speedup_ratio": round(cargo_cold["wall_sec"] / max(mitts_cold["wall_sec"], 0.001), 2),
        "disk_savings_ratio": round(cargo_cold_disk / max(mitts_cold_disk, 0.001), 2)
    }

    # -------------------------------------------------------------
    # 2. Single-Worktree Warm No-Op Build
    # -------------------------------------------------------------
    print(f"\n--- [2] Single-Worktree Warm No-Op Build ---")
    print(f"Running Cargo Warm No-Op...")
    cargo_warm = measure_command(["cargo", "build"], single_wt, env=cargo_env)
    print(f"  -> Cargo Warm: {cargo_warm['wall_sec']}s | CPU: {cargo_warm['cpu_percent']}%")

    print(f"Running Mitts Warm No-Op (Fast Lock)...")
    mitts_warm = measure_command([str(MITTS_BIN), "build"], single_wt, env=mitts_env)
    print(f"  -> Mitts Warm: {mitts_warm['wall_sec']}s | CPU: {mitts_warm['cpu_percent']}%")

    results["single_warm_noop"] = {
        "cargo": cargo_warm,
        "mitts": mitts_warm,
        "speedup_ratio": round(cargo_warm["wall_sec"] / max(mitts_warm["wall_sec"], 0.0001), 2)
    }

    # -------------------------------------------------------------
    # Setup 30 Worktrees
    # -------------------------------------------------------------
    worktrees = prepare_worktrees(project_name, project_repo_dir, worktree_base_dir, num_worktrees=30)

    # -------------------------------------------------------------
    # 3. 30-Worktree Concurrent Cold Build (Swarm)
    # -------------------------------------------------------------
    print(f"\n--- [3] 30-Worktree Concurrent Cold Swarm ---")
    if shared_cas.exists():
        shutil.rmtree(shared_cas)
    for _, wt_dir in worktrees:
        m_dir = wt_dir / ".mitts"
        if m_dir.exists():
            shutil.rmtree(m_dir)

    def mitts_cold_swarm_fn(wt):
        _, wt_dir = wt
        return measure_command([str(MITTS_BIN), "build"], wt_dir, env=mitts_env)

    print(f"Running Mitts 30x Concurrent Cold...")
    mitts_swarm_cold = run_parallel_builds(mitts_cold_swarm_fn, worktrees)
    mitts_swarm_cold_disk = sum(get_dir_size_mb(wt_dir / ".mitts") for _, wt_dir in worktrees) + get_dir_size_mb(shared_cas)
    mitts_swarm_cold["disk_written_mb"] = round(mitts_swarm_cold_disk, 1)
    print(f"  -> Mitts 30x Cold: {mitts_swarm_cold['wall_sec']}s | CPU: {mitts_swarm_cold['cpu_percent']}% | Disk: {mitts_swarm_cold_disk:.1f}MB")

    for _, wt_dir in worktrees:
        c_dir = wt_dir / "target"
        if c_dir.exists():
            shutil.rmtree(c_dir)

    def cargo_cold_swarm_fn(wt):
        _, wt_dir = wt
        return measure_command(["cargo", "build"], wt_dir, env=cargo_env)

    print(f"Running Cargo 30x Concurrent Cold...")
    cargo_swarm_cold = run_parallel_builds(cargo_cold_swarm_fn, worktrees)
    cargo_swarm_cold_disk = sum(get_dir_size_mb(wt_dir / "target") for _, wt_dir in worktrees)
    cargo_swarm_cold["disk_written_mb"] = round(cargo_swarm_cold_disk, 1)
    print(f"  -> Cargo 30x Cold: {cargo_swarm_cold['wall_sec']}s | CPU: {cargo_swarm_cold['cpu_percent']}% | Disk: {cargo_swarm_cold_disk:.1f}MB")

    results["swarm_cold_30x"] = {
        "cargo": cargo_swarm_cold,
        "mitts": mitts_swarm_cold,
        "speedup_ratio": round(cargo_swarm_cold["wall_sec"] / max(mitts_swarm_cold["wall_sec"], 0.001), 2),
        "disk_savings_ratio": round(cargo_swarm_cold_disk / max(mitts_swarm_cold_disk, 0.001), 2)
    }

    # -------------------------------------------------------------
    # 4. 30-Worktree Concurrent Warm Build (Populated CAS vs Cargo from Scratch)
    # -------------------------------------------------------------
    print(f"\n--- [4] 30-Worktree Concurrent Warm Build (Clean WT target, Populated CAS) ---")
    for _, wt_dir in worktrees:
        m_dir = wt_dir / ".mitts"
        if m_dir.exists():
            shutil.rmtree(m_dir)

    print(f"Running Mitts 30x Concurrent Warm (Instant CAS linking)...")
    mitts_swarm_warm = run_parallel_builds(mitts_cold_swarm_fn, worktrees)
    mitts_swarm_warm_disk = sum(get_dir_size_mb(wt_dir / ".mitts") for _, wt_dir in worktrees)
    mitts_swarm_warm["disk_written_mb"] = round(mitts_swarm_warm_disk, 1)
    print(f"  -> Mitts 30x Warm: {mitts_swarm_warm['wall_sec']}s | CPU: {mitts_swarm_warm['cpu_percent']}% | Disk: {mitts_swarm_warm_disk:.1f}MB")

    results["swarm_warm_30x"] = {
        "cargo": cargo_swarm_cold, # Cargo has no cross-worktree cache, must rebuild
        "mitts": mitts_swarm_warm,
        "speedup_ratio": round(cargo_swarm_cold["wall_sec"] / max(mitts_swarm_warm["wall_sec"], 0.001), 2),
        "disk_savings_ratio": round(cargo_swarm_cold_disk / max(mitts_swarm_warm_disk, 0.001), 2)
    }

    return results

def main():
    projects = [
        ("serde", PROJECTS_DIR / "serde", BENCH_ROOT / "worktrees_stress"),
        ("tokio", PROJECTS_DIR / "tokio", BENCH_ROOT / "tokio_swarm_30"),
        ("ripgrep", PROJECTS_DIR / "ripgrep", BENCH_ROOT / "ripgrep_swarm_30")
    ]
    
    full_report = {
        "metadata": {
            "timestamp": int(time.time()),
            "host": "booster (64 cores, 128 GB RAM, ZFS)",
            "mitts_version": "0.1.0-release",
            "worktrees_count": 30
        },
        "projects": {}
    }

    for name, p_dir, wt_dir in projects:
        try:
            full_report["projects"][name] = benchmark_project(name, p_dir, wt_dir)
        except Exception as e:
            print(f"Error benchmarking {name}: {e}")
            import traceback
            traceback.print_exc()

    out_file = BENCH_ROOT / "benchmark_results_v2.json"
    with open(out_file, "w") as f:
        json.dump(full_report, f, indent=2)
    print(f"\n✅ All benchmarks finished! Results saved to {out_file}")

if __name__ == "__main__":
    main()
