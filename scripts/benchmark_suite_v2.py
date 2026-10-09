#!/usr/bin/env python3
import concurrent.futures
import json
import os
import shutil
import subprocess
import sys
import time
from pathlib import Path

# Paths
CARGO_BIN_DIR = str(Path.home() / ".cargo" / "bin")
if CARGO_BIN_DIR not in os.environ.get("PATH", ""):
    os.environ["PATH"] = f"{CARGO_BIN_DIR}:{os.environ.get('PATH', '')}"

BENCH_ROOT = Path(os.environ.get("BENCH_ROOT", Path.home() / "mitts-benchmarks"))
PROJECTS_DIR = Path(os.environ.get("PROJECTS_DIR", BENCH_ROOT / "projects"))
MITTS_BIN = Path(
    os.environ.get(
        "MITTS_BIN",
        shutil.which("mitts") or (BENCH_ROOT / "mitts-bin")
    )
)
SCCACHE_BIN = Path(shutil.which("sccache") or (Path.home() / ".cargo/bin/sccache"))

def measure_command(cmd, cwd, env=None):
    if env is None:
        env = os.environ.copy()
    else:
        env = env.copy()
    if CARGO_BIN_DIR not in env.get("PATH", ""):
        env["PATH"] = f"{CARGO_BIN_DIR}:{env.get('PATH', '')}"

    start = time.perf_counter()
    p = subprocess.Popen(
        cmd,
        cwd=cwd,
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True
    )
    # wait4 captures child and grandchild processes (rustc, linkers, etc)
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

def clean_sccache(sccache_dir):
    try:
        subprocess.run([str(SCCACHE_BIN), "--stop-server"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    except Exception:
        pass
    if sccache_dir.exists():
        shutil.rmtree(sccache_dir, ignore_errors=True)
    sccache_dir.mkdir(parents=True, exist_ok=True)
    env = os.environ.copy()
    env["SCCACHE_DIR"] = str(sccache_dir)
    try:
        subprocess.run([str(SCCACHE_BIN), "--start-server"], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        subprocess.run([str(SCCACHE_BIN), "--zero-stats"], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    except Exception:
        pass

def benchmark_project(project_name, project_repo_dir, worktree_base_dir):
    print(f"\n=======================================================")
    print(f"🚀 BENCHMARKING: {project_name.upper()} (3-Way: Cargo vs Sccache vs Mitts)")
    print(f"=======================================================")
    results = {}
    
    single_wt = project_repo_dir
    shared_cas = BENCH_ROOT / f"cas_bench_{project_name}"
    sccache_dir = BENCH_ROOT / f"sccache_bench_{project_name}"
    
    # Environment setups
    cargo_env = os.environ.copy()
    cargo_env["RUSTC_WRAPPER"] = ""
    
    sccache_env = os.environ.copy()
    sccache_env["RUSTC_WRAPPER"] = str(SCCACHE_BIN)
    sccache_env["SCCACHE_DIR"] = str(sccache_dir)
    sccache_env["CARGO_INCREMENTAL"] = "0"
    
    mitts_env = os.environ.copy()
    mitts_env["MITTS_CACHE_DIR"] = str(shared_cas)

    # -------------------------------------------------------------
    # 1. Single-Worktree Cold Build
    # -------------------------------------------------------------
    print(f"\n--- [1] Single-Worktree Cold Build ---")
    
    # 1A. Pure Cargo Cold
    subprocess.run(["cargo", "clean"], cwd=single_wt, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    print(f"Running Cargo Single Cold...")
    cargo_cold = measure_command(["cargo", "build"], single_wt, env=cargo_env)
    cargo_cold_disk = get_dir_size_mb(single_wt / "target")
    cargo_cold["disk_written_mb"] = cargo_cold_disk
    print(f"  -> Cargo Cold: {cargo_cold['wall_sec']}s | CPU: {cargo_cold['cpu_percent']}% | Disk: {cargo_cold_disk}MB")

    # 1B. Cargo + sccache Cold
    subprocess.run(["cargo", "clean"], cwd=single_wt, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    clean_sccache(sccache_dir)
    print(f"Running Sccache Single Cold...")
    sccache_cold = measure_command(["cargo", "build"], single_wt, env=sccache_env)
    sccache_cold_disk = get_dir_size_mb(single_wt / "target") + get_dir_size_mb(sccache_dir)
    sccache_cold["disk_written_mb"] = sccache_cold_disk
    print(f"  -> Sccache Cold: {sccache_cold['wall_sec']}s | CPU: {sccache_cold['cpu_percent']}% | Disk: {sccache_cold_disk}MB")

    # 1C. Mitts Single Cold
    if shared_cas.exists():
        shutil.rmtree(shared_cas)
    if (single_wt / ".mitts").exists():
        shutil.rmtree(single_wt / ".mitts")
    print(f"Running Mitts Single Cold...")
    mitts_cold = measure_command([str(MITTS_BIN), "build"], single_wt, env=mitts_env)
    mitts_cold_disk = get_dir_size_mb(single_wt / ".mitts") + get_dir_size_mb(shared_cas)
    mitts_cold["disk_written_mb"] = mitts_cold_disk
    print(f"  -> Mitts Cold: {mitts_cold['wall_sec']}s | CPU: {mitts_cold['cpu_percent']}% | Disk: {mitts_cold_disk}MB")
    
    results["single_cold"] = {
        "cargo": cargo_cold,
        "sccache": sccache_cold,
        "mitts": mitts_cold,
        "speedup_vs_cargo": round(cargo_cold["wall_sec"] / max(mitts_cold["wall_sec"], 0.001), 2),
        "speedup_vs_sccache": round(sccache_cold["wall_sec"] / max(mitts_cold["wall_sec"], 0.001), 2),
        "disk_savings_vs_cargo": round(cargo_cold_disk / max(mitts_cold_disk, 0.001), 2)
    }

    # -------------------------------------------------------------
    # 2. Single-Worktree Warm Build
    # -------------------------------------------------------------
    print(f"\n--- [2] Single-Worktree Warm Build ---")
    # Cargo Warm (no-op check)
    print(f"Running Cargo Warm No-Op...")
    cargo_warm = measure_command(["cargo", "build"], single_wt, env=cargo_env)
    print(f"  -> Cargo Warm: {cargo_warm['wall_sec']}s | CPU: {cargo_warm['cpu_percent']}%")

    # Sccache Warm (clean target, warm sccache cache)
    subprocess.run(["cargo", "clean"], cwd=single_wt, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    print(f"Running Sccache Warm (Clean Target + Warm Sccache Cache)...")
    sccache_warm = measure_command(["cargo", "build"], single_wt, env=sccache_env)
    print(f"  -> Sccache Warm: {sccache_warm['wall_sec']}s | CPU: {sccache_warm['cpu_percent']}%")

    # Mitts Warm (fast-lock no-op)
    print(f"Running Mitts Warm No-Op (Fast Lock)...")
    mitts_warm = measure_command([str(MITTS_BIN), "build"], single_wt, env=mitts_env)
    print(f"  -> Mitts Warm: {mitts_warm['wall_sec']}s | CPU: {mitts_warm['cpu_percent']}%")

    results["single_warm"] = {
        "cargo": cargo_warm,
        "sccache": sccache_warm,
        "mitts": mitts_warm,
        "speedup_vs_cargo": round(cargo_warm["wall_sec"] / max(mitts_warm["wall_sec"], 0.0001), 2),
        "speedup_vs_sccache": round(sccache_warm["wall_sec"] / max(mitts_warm["wall_sec"], 0.0001), 2)
    }

    # -------------------------------------------------------------
    # Setup 30 Worktrees
    # -------------------------------------------------------------
    worktrees = prepare_worktrees(project_name, project_repo_dir, worktree_base_dir, num_worktrees=30)

    # -------------------------------------------------------------
    # 3. 30-Worktree Concurrent Cold Build (Swarm)
    # -------------------------------------------------------------
    print(f"\n--- [3] 30-Worktree Concurrent Cold Swarm ---")
    
    # 3A. Mitts 30x Cold
    if shared_cas.exists():
        shutil.rmtree(shared_cas)
    for _, wt_dir in worktrees:
        m_dir = wt_dir / ".mitts"
        if m_dir.exists():
            shutil.rmtree(m_dir)

    def mitts_cmd_fn(wt):
        _, wt_dir = wt
        return measure_command([str(MITTS_BIN), "build"], wt_dir, env=mitts_env)

    print(f"Running Mitts 30x Concurrent Cold (TryLock Coordination)...")
    mitts_swarm_cold = run_parallel_builds(mitts_cmd_fn, worktrees)
    mitts_swarm_cold_disk = sum(get_dir_size_mb(wt_dir / ".mitts") for _, wt_dir in worktrees) + get_dir_size_mb(shared_cas)
    mitts_swarm_cold["disk_written_mb"] = round(mitts_swarm_cold_disk, 1)
    print(f"  -> Mitts 30x Cold: {mitts_swarm_cold['wall_sec']}s | CPU: {mitts_swarm_cold['cpu_percent']}% | Disk: {mitts_swarm_cold_disk:.1f}MB")

    # 3B. Cargo + sccache 30x Cold
    clean_sccache(sccache_dir)
    for _, wt_dir in worktrees:
        c_dir = wt_dir / "target"
        if c_dir.exists():
            shutil.rmtree(c_dir)

    def sccache_cmd_fn(wt):
        _, wt_dir = wt
        return measure_command(["cargo", "build"], wt_dir, env=sccache_env)

    print(f"Running Sccache 30x Concurrent Cold...")
    sccache_swarm_cold = run_parallel_builds(sccache_cmd_fn, worktrees)
    sccache_swarm_cold_disk = sum(get_dir_size_mb(wt_dir / "target") for _, wt_dir in worktrees) + get_dir_size_mb(sccache_dir)
    sccache_swarm_cold["disk_written_mb"] = round(sccache_swarm_cold_disk, 1)
    print(f"  -> Sccache 30x Cold: {sccache_swarm_cold['wall_sec']}s | CPU: {sccache_swarm_cold['cpu_percent']}% | Disk: {sccache_swarm_cold_disk:.1f}MB")

    # 3C. Pure Cargo 30x Cold
    for _, wt_dir in worktrees:
        c_dir = wt_dir / "target"
        if c_dir.exists():
            shutil.rmtree(c_dir)

    def cargo_cmd_fn(wt):
        _, wt_dir = wt
        return measure_command(["cargo", "build"], wt_dir, env=cargo_env)

    print(f"Running Cargo 30x Concurrent Cold (Uncoordinated)...")
    cargo_swarm_cold = run_parallel_builds(cargo_cmd_fn, worktrees)
    cargo_swarm_cold_disk = sum(get_dir_size_mb(wt_dir / "target") for _, wt_dir in worktrees)
    cargo_swarm_cold["disk_written_mb"] = round(cargo_swarm_cold_disk, 1)
    print(f"  -> Cargo 30x Cold: {cargo_swarm_cold['wall_sec']}s | CPU: {cargo_swarm_cold['cpu_percent']}% | Disk: {cargo_swarm_cold_disk:.1f}MB")

    results["swarm_cold_30x"] = {
        "cargo": cargo_swarm_cold,
        "sccache": sccache_swarm_cold,
        "mitts": mitts_swarm_cold,
        "speedup_vs_cargo": round(cargo_swarm_cold["wall_sec"] / max(mitts_swarm_cold["wall_sec"], 0.001), 2),
        "speedup_vs_sccache": round(sccache_swarm_cold["wall_sec"] / max(mitts_swarm_cold["wall_sec"], 0.001), 2),
        "disk_savings_vs_cargo": round(cargo_swarm_cold_disk / max(mitts_swarm_cold_disk, 0.001), 2),
        "disk_savings_vs_sccache": round(sccache_swarm_cold_disk / max(mitts_swarm_cold_disk, 0.001), 2)
    }

    # -------------------------------------------------------------
    # 4. 30-Worktree Concurrent Warm Build (Warm Cache Restoration)
    # -------------------------------------------------------------
    print(f"\n--- [4] 30-Worktree Concurrent Warm Build (Populated Caches) ---")
    
    # 4A. Mitts Warm Swarm (Clean .mitts, warm CAS)
    for _, wt_dir in worktrees:
        m_dir = wt_dir / ".mitts"
        if m_dir.exists():
            shutil.rmtree(m_dir)

    print(f"Running Mitts 30x Warm (Instant Reflink/Hardlink from CAS)...")
    mitts_swarm_warm = run_parallel_builds(mitts_cmd_fn, worktrees)
    mitts_swarm_warm_disk = sum(get_dir_size_mb(wt_dir / ".mitts") for _, wt_dir in worktrees)
    mitts_swarm_warm["disk_written_mb"] = round(mitts_swarm_warm_disk, 1)
    print(f"  -> Mitts 30x Warm: {mitts_swarm_warm['wall_sec']}s | CPU: {mitts_swarm_warm['cpu_percent']}% | Disk: {mitts_swarm_warm_disk:.1f}MB")

    # 4B. Sccache Warm Swarm (Clean target, warm Sccache cache)
    for _, wt_dir in worktrees:
        c_dir = wt_dir / "target"
        if c_dir.exists():
            shutil.rmtree(c_dir)

    print(f"Running Sccache 30x Warm (Warm Sccache Cache)...")
    sccache_swarm_warm = run_parallel_builds(sccache_cmd_fn, worktrees)
    sccache_swarm_warm_disk = sum(get_dir_size_mb(wt_dir / "target") for _, wt_dir in worktrees)
    sccache_swarm_warm["disk_written_mb"] = round(sccache_swarm_warm_disk, 1)
    print(f"  -> Sccache 30x Warm: {sccache_swarm_warm['wall_sec']}s | CPU: {sccache_swarm_warm['cpu_percent']}% | Disk: {sccache_swarm_warm_disk:.1f}MB")

    # 4C. Cargo: Cargo has no cross-worktree cache, so warm swarm equals clean rebuild
    cargo_swarm_warm = cargo_swarm_cold

    results["swarm_warm_30x"] = {
        "cargo": cargo_swarm_warm,
        "sccache": sccache_swarm_warm,
        "mitts": mitts_swarm_warm,
        "speedup_vs_cargo": round(cargo_swarm_warm["wall_sec"] / max(mitts_swarm_warm["wall_sec"], 0.001), 2),
        "speedup_vs_sccache": round(sccache_swarm_warm["wall_sec"] / max(mitts_swarm_warm["wall_sec"], 0.001), 2),
        "disk_savings_vs_cargo": round(cargo_swarm_cold_disk / max(mitts_swarm_warm_disk, 0.001), 2),
        "disk_savings_vs_sccache": round(sccache_swarm_warm_disk / max(mitts_swarm_warm_disk, 0.001), 2)
    }

    return results

def main():
    projects = [
        ("serde", PROJECTS_DIR / "serde", BENCH_ROOT / "worktrees_stress"),
        ("tokio", PROJECTS_DIR / "tokio", BENCH_ROOT / "tokio_swarm_30"),
        ("ripgrep", PROJECTS_DIR / "ripgrep", BENCH_ROOT / "ripgrep_swarm_30"),
        ("clap", PROJECTS_DIR / "clap", BENCH_ROOT / "clap_swarm_30"),
        ("ring", PROJECTS_DIR / "ring", BENCH_ROOT / "ring_swarm_30")
    ]
    
    if len(sys.argv) > 1:
        selected = sys.argv[1].split(",")
        projects = [p for p in projects if p[0] in selected]

    out_file = BENCH_ROOT / "benchmark_results_v2.json"
    full_report = {}
    if out_file.exists():
        try:
            with open(out_file, "r") as f:
                full_report = json.load(f)
        except Exception:
            full_report = {}

    if "metadata" not in full_report:
        full_report["metadata"] = {
            "timestamp": int(time.time()),
            "host": "benchmark host (AMD Ryzen 9 9950X Zen 5, 32 threads, 192 GB DDR5 RAM, Ubuntu 24.04)",
            "mitts_version": "0.1.0-release",
            "worktrees_count": 30,
            "engines": ["cargo", "cargo_sccache", "mitts"]
        }
    full_report["metadata"]["timestamp"] = int(time.time())
    if "projects" not in full_report:
        full_report["projects"] = {}

    for name, p_dir, wt_dir in projects:
        try:
            full_report["projects"][name] = benchmark_project(name, p_dir, wt_dir)
        except Exception as e:
            print(f"Error benchmarking {name}: {e}")
            import traceback
            traceback.print_exc()

    with open(out_file, "w") as f:
        json.dump(full_report, f, indent=2)
    print(f"\n✅ All benchmarks finished! Results saved to {out_file}")

if __name__ == "__main__":
    main()
