#!/usr/bin/env python3
"""
Mitts Multi-Project Benchmark Matrix Runner
Evaluates 4 build systems across 5 distinct scenarios:
  1. Baseline A: Pure Cargo (--offline)
  2. Baseline B: Cargo + Shared CARGO_TARGET_DIR (--offline)
  3. Baseline C: Cargo + sccache (CARGO_INCREMENTAL=0, --offline)
  4. Mitts: mitts build

Scenarios:
  W1a: Fast No-Op (source unchanged)
  W1b: Invalidation No-Op (touch new file src/bin/probe.rs or src/probe.rs, rebuild, clean up)
  W2: Single Cold Build (strictly purged cache)
  W3: Cross-Worktree Warm Restoration (worktree 1 populates cache; worktree 2 builds with clean local target)
  W4: 30x Concurrent Swarm (Cold & Warm)
  W5: Parity Gate (Exit code, binary execution, test output)
"""

import concurrent.futures
import json
import os
import shutil
import subprocess
import sys
import time
from pathlib import Path
import statistics

# Environment configuration
CARGO_BIN_DIR = str(Path.home() / ".cargo" / "bin")
if CARGO_BIN_DIR not in os.environ.get("PATH", ""):
    os.environ["PATH"] = f"{CARGO_BIN_DIR}:{os.environ.get('PATH', '')}"

CANDIDATE_MITTS = [
    Path(os.environ["MITTS_BIN"]) if "MITTS_BIN" in os.environ else None,
    Path.home() / "mitts/target/release/mitts",
]
VALID_CANDIDATE_MITTS = [p for p in CANDIDATE_MITTS if p is not None]
MITTS_BIN = next((p for p in VALID_CANDIDATE_MITTS if p.exists()), VALID_CANDIDATE_MITTS[0] if VALID_CANDIDATE_MITTS else Path("mitts"))
SCCACHE_BIN = Path(shutil.which("sccache") or (Path.home() / ".cargo/bin/sccache"))
REPO_ROOT = Path(__file__).resolve().parent.parent
BENCHMARK_RESULTS_DIR = Path(
    os.environ.get("BENCHMARK_OUTPUT_DIR", REPO_ROOT / "benchmark-results")
)

BENCH_ROOT = Path(os.environ.get("BENCH_ROOT") or (Path.home() / "mitts-benchmarks"))
PROJECTS_DIR = BENCH_ROOT / "projects"
SHARED_TARGET_ROOT = BENCH_ROOT / "shared_cargo_target"


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
    # os.wait4 collects child and grandchild process statistics (rustc, linkers, etc.)
    _, status, ru = os.wait4(p.pid, 0)
    stdout, stderr = p.communicate()
    elapsed = time.perf_counter() - start

    exit_code = os.waitstatus_to_exitcode(status) if hasattr(os, "waitstatus_to_exitcode") else (status >> 8)
    utime = ru.ru_utime
    stime = ru.ru_stime
    total_cpu = utime + stime
    cpu_percent = (total_cpu / max(elapsed, 0.001)) * 100.0
    max_rss_mb = ru.ru_maxrss / 1024.0

    return {
        "wall_sec": round(elapsed, 3),
        "user_sec": round(utime, 3),
        "sys_sec": round(stime, 3),
        "total_cpu_sec": round(total_cpu, 3),
        "cpu_percent": round(cpu_percent, 1),
        "max_rss_mb": round(max_rss_mb, 1),
        "exit_code": exit_code,
        "stdout_tail": stdout[-300:] if stdout else "",
        "stderr_tail": stderr[-300:] if stderr else ""
    }


def get_dir_size_mb(path):
    p = Path(path)
    if not p.exists():
        return 0.0
    total = 0
    for root, _, files in os.walk(p):
        for f in files:
            fp = os.path.join(root, f)
            if not os.path.islink(fp):
                try:
                    total += os.path.getsize(fp)
                except OSError:
                    pass
    return round(total / (1024.0 * 1024.0), 2)


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


def prepare_worktrees(project_name, repo_dir, worktree_base_dir, num_worktrees=30):
    worktree_base_dir.mkdir(parents=True, exist_ok=True)
    worktrees = []
    existing_wts = sorted([d for d in worktree_base_dir.glob("wt-*") if d.is_dir()])
    if len(existing_wts) >= num_worktrees:
        for i, wt_dir in enumerate(existing_wts[:num_worktrees], start=1):
            worktrees.append((i, wt_dir))
        return worktrees

    print(f"[{project_name}] Preparing {num_worktrees} git worktrees...")
    for i in range(1, num_worktrees + 1):
        wt_dir = worktree_base_dir / f"wt-{i:02d}"
        if not wt_dir.exists():
            cmd = ["git", "worktree", "add", "-d", str(wt_dir), "HEAD"]
            subprocess.run(cmd, cwd=repo_dir, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        worktrees.append((i, wt_dir))
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
    print(f"\n{'='*70}")
    print(f"🚀 BENCHMARKING: {project_name.upper()} (4-Way Matrix on Ryzen 9 9950X)")
    print(f"{'='*70}")

    results = {}
    single_wt = project_repo_dir
    shared_cas = BENCH_ROOT / f"cas_bench_{project_name}"
    sccache_dir = BENCH_ROOT / f"sccache_bench_{project_name}"
    shared_target_dir = SHARED_TARGET_ROOT / project_name
    shared_target_dir.mkdir(parents=True, exist_ok=True)

    # Pre-fetch offline assurance
    subprocess.run(["cargo", "fetch"], cwd=single_wt, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

    # Base environments
    env_cargo_bare = os.environ.copy()
    env_cargo_bare["RUSTC_WRAPPER"] = ""

    env_cargo_shared = os.environ.copy()
    env_cargo_shared["CARGO_TARGET_DIR"] = str(shared_target_dir)
    env_cargo_shared["RUSTC_WRAPPER"] = ""

    env_sccache = os.environ.copy()
    env_sccache["RUSTC_WRAPPER"] = str(SCCACHE_BIN)
    env_sccache["SCCACHE_DIR"] = str(sccache_dir)
    env_sccache["CARGO_INCREMENTAL"] = "0"

    env_mitts = os.environ.copy()
    env_mitts["MITTS_CACHE_DIR"] = str(shared_cas)

    # -------------------------------------------------------------
    # W2: Single-Worktree Cold Build (3 runs with clean caches)
    # -------------------------------------------------------------
    print(f"\n--- [W2] Single Cold Build (Rigorous Clean Cache) ---")

    def run_cold_cargo_bare():
        subprocess.run(["cargo", "clean"], cwd=single_wt, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        return measure_command(["cargo", "build", "--offline"], single_wt, env=env_cargo_bare)

    def run_cold_cargo_shared():
        if shared_target_dir.exists():
            shutil.rmtree(shared_target_dir)
        shared_target_dir.mkdir(parents=True, exist_ok=True)
        return measure_command(["cargo", "build", "--offline"], single_wt, env=env_cargo_shared)

    def run_cold_sccache():
        subprocess.run(["cargo", "clean"], cwd=single_wt, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        clean_sccache(sccache_dir)
        return measure_command(["cargo", "build", "--offline"], single_wt, env=env_sccache)

    def run_cold_mitts():
        if shared_cas.exists():
            shutil.rmtree(shared_cas)
        user_cache = Path.home() / ".cache" / "mitts"
        if user_cache.exists():
            shutil.rmtree(user_cache)
        if (single_wt / ".mitts").exists():
            shutil.rmtree(single_wt / ".mitts")
        assert not shared_cas.exists(), "shared_cas must be empty"
        assert not (single_wt / ".mitts").exists(), ".mitts must be empty"
        return measure_command([str(MITTS_BIN), "build"], single_wt, env=env_mitts)

    print("Running Cold Mitts...")
    mitts_cold = run_cold_mitts()
    mitts_cold_disk = get_dir_size_mb(single_wt / ".mitts") + get_dir_size_mb(shared_cas)
    mitts_cold["disk_mb"] = mitts_cold_disk
    print(f"  -> Mitts Cold: {mitts_cold['wall_sec']}s | CPU: {mitts_cold['cpu_percent']}% | Disk: {mitts_cold_disk}MB | Exit: {mitts_cold['exit_code']}")

    print("Running Cold Cargo Bare...")
    cargo_bare_cold = run_cold_cargo_bare()
    cargo_bare_disk = get_dir_size_mb(single_wt / "target")
    cargo_bare_cold["disk_mb"] = cargo_bare_disk
    print(f"  -> Cargo Bare Cold: {cargo_bare_cold['wall_sec']}s | CPU: {cargo_bare_cold['cpu_percent']}% | Disk: {cargo_bare_disk}MB | Exit: {cargo_bare_cold['exit_code']}")

    print("Running Cold Cargo + Shared Target...")
    cargo_shared_cold = run_cold_cargo_shared()
    cargo_shared_disk = get_dir_size_mb(shared_target_dir)
    cargo_shared_cold["disk_mb"] = cargo_shared_disk
    print(f"  -> Cargo Shared Cold: {cargo_shared_cold['wall_sec']}s | CPU: {cargo_shared_cold['cpu_percent']}% | Disk: {cargo_shared_disk}MB | Exit: {cargo_shared_cold['exit_code']}")

    print("Running Cold Sccache...")
    sccache_cold = run_cold_sccache()
    sccache_disk = get_dir_size_mb(single_wt / "target") + get_dir_size_mb(sccache_dir)
    sccache_cold["disk_mb"] = sccache_disk
    print(f"  -> Sccache Cold: {sccache_cold['wall_sec']}s | CPU: {sccache_cold['cpu_percent']}% | Disk: {sccache_disk}MB | Exit: {sccache_cold['exit_code']}")

    # -------------------------------------------------------------
    # W1a: Fast No-Op Build (Unchanged sources, warm caches)
    # -------------------------------------------------------------
    print(f"\n--- [W1a] Fast No-Op (Unchanged Sources, Warm Fingerprints) ---")
    # Repopulate targets
    run_cold_cargo_bare()
    cargo_bare_noop = measure_command(["cargo", "build", "--offline"], single_wt, env=env_cargo_bare)
    print(f"  -> Cargo Bare No-Op: {cargo_bare_noop['wall_sec']}s | CPU: {cargo_bare_noop['cpu_percent']}%")

    cargo_shared_noop = measure_command(["cargo", "build", "--offline"], single_wt, env=env_cargo_shared)
    print(f"  -> Cargo Shared No-Op: {cargo_shared_noop['wall_sec']}s | CPU: {cargo_shared_noop['cpu_percent']}%")

    sccache_noop = measure_command(["cargo", "build", "--offline"], single_wt, env=env_sccache)
    print(f"  -> Sccache No-Op: {sccache_noop['wall_sec']}s | CPU: {sccache_noop['cpu_percent']}%")

    # Mitts no-op: compile once to ensure warm fast lock, then measure
    measure_command([str(MITTS_BIN), "build"], single_wt, env=env_mitts)
    mitts_noop = measure_command([str(MITTS_BIN), "build"], single_wt, env=env_mitts)
    print(f"  -> Mitts Fast No-Op: {mitts_noop['wall_sec']}s | CPU: {mitts_noop['cpu_percent']}%")

    # -------------------------------------------------------------
    # W1b: Invalidation No-Op (Touch added file, rebuild, clean up)
    # -------------------------------------------------------------
    print(f"\n--- [W1b] Added File Invalidation (touch probe.rs) ---")
    src_dir = None
    if (single_wt / "src").is_dir():
        src_dir = single_wt / "src"
    else:
        for p in single_wt.glob("**/src"):
            if p.is_dir() and "target" not in str(p) and ".mitts" not in str(p):
                src_dir = p
                break

    if src_dir and src_dir.exists():
        probe_file = src_dir / "probe.rs"
        probe_file.write_text("// temporary probe file\npub fn __probe_marker() {}\n")
        cargo_bare_inval = measure_command(["cargo", "build", "--offline"], single_wt, env=env_cargo_bare)
        mitts_inval = measure_command([str(MITTS_BIN), "build"], single_wt, env=env_mitts)
        if probe_file.exists():
            probe_file.unlink()
        print(f"  -> Cargo Bare Added-File Rebuild: {cargo_bare_inval['wall_sec']}s | Exit: {cargo_bare_inval['exit_code']}")
        print(f"  -> Mitts Added-File Rebuild: {mitts_inval['wall_sec']}s | Exit: {mitts_inval['exit_code']}")
    else:
        print("  -> Skipped W1b: no suitable src directory found")

    # -------------------------------------------------------------
    # W3: Cross-Worktree Warm Restoration (WT1 populates -> WT2 builds clean)
    # -------------------------------------------------------------
    print(f"\n--- [W3] Cross-Worktree Warm Restoration ---")
    worktrees = prepare_worktrees(project_name, project_repo_dir, worktree_base_dir, num_worktrees=30)
    wt1 = worktrees[0][1]
    wt2 = worktrees[1][1]

    # Clean wt2 target directories
    if (wt2 / "target").exists():
        shutil.rmtree(wt2 / "target")
    if (wt2 / ".mitts").exists():
        shutil.rmtree(wt2 / ".mitts")

    # 1. Cargo Bare: WT2 has no cross-worktree cache
    cargo_bare_wt2 = measure_command(["cargo", "build", "--offline"], wt2, env=env_cargo_bare)
    print(f"  -> Cargo Bare WT2 Rebuild: {cargo_bare_wt2['wall_sec']}s | Exit: {cargo_bare_wt2['exit_code']}")

    # 2. Cargo Shared Target: WT2 reuses shared target directory
    cargo_shared_wt2 = measure_command(["cargo", "build", "--offline"], wt2, env=env_cargo_shared)
    print(f"  -> Cargo Shared WT2 Reuse: {cargo_shared_wt2['wall_sec']}s | Exit: {cargo_shared_wt2['exit_code']}")

    # 3. Sccache: WT2 fetches objects from sccache
    sccache_wt2 = measure_command(["cargo", "build", "--offline"], wt2, env=env_sccache)
    print(f"  -> Sccache WT2 Restoration: {sccache_wt2['wall_sec']}s | Exit: {sccache_wt2['exit_code']}")

    # 4. Mitts: WT2 restores instant hardlinks from shared CAS
    mitts_wt2 = measure_command([str(MITTS_BIN), "build"], wt2, env=env_mitts)
    print(f"  -> Mitts WT2 CAS Restoration: {mitts_wt2['wall_sec']}s | Exit: {mitts_wt2['exit_code']}")

    # -------------------------------------------------------------
    # W4: 30-Worktree Concurrent Swarm (Cold & Warm)
    # -------------------------------------------------------------
    print(f"\n--- [W4] 30-Worktree Concurrent Swarm ---")

    # 4A. Mitts Warm Swarm
    for _, wt_dir in worktrees:
        m_dir = wt_dir / ".mitts"
        if m_dir.exists():
            shutil.rmtree(m_dir)

    def mitts_cmd(wt):
        return measure_command([str(MITTS_BIN), "build"], wt[1], env=env_mitts)

    print("Running Mitts 30x Warm Swarm (Coordinated TryLock CAS link)...")
    mitts_swarm_warm = run_parallel_builds(mitts_cmd, worktrees)
    mitts_swarm_disk = sum(get_dir_size_mb(w[1] / ".mitts") for w in worktrees) + get_dir_size_mb(shared_cas)
    mitts_swarm_warm["disk_mb"] = round(mitts_swarm_disk, 1)
    print(f"  -> Mitts 30x Warm: {mitts_swarm_warm['wall_sec']}s | CPU: {mitts_swarm_warm['cpu_percent']}% | Disk: {mitts_swarm_disk:.1f}MB | Pass: {mitts_swarm_warm['success_count']}/30")

    # 4B. Sccache Warm Swarm
    for _, wt_dir in worktrees:
        c_dir = wt_dir / "target"
        if c_dir.exists():
            shutil.rmtree(c_dir)

    def sccache_cmd(wt):
        return measure_command(["cargo", "build", "--offline"], wt[1], env=env_sccache)

    print("Running Sccache 30x Warm Swarm...")
    sccache_swarm_warm = run_parallel_builds(sccache_cmd, worktrees)
    sccache_swarm_disk = sum(get_dir_size_mb(w[1] / "target") for w in worktrees) + get_dir_size_mb(sccache_dir)
    sccache_swarm_warm["disk_mb"] = round(sccache_swarm_disk, 1)
    print(f"  -> Sccache 30x Warm: {sccache_swarm_warm['wall_sec']}s | CPU: {sccache_swarm_warm['cpu_percent']}% | Disk: {sccache_swarm_disk:.1f}MB | Pass: {sccache_swarm_warm['success_count']}/30")

    # 4C. Cargo Shared Target Warm Swarm (Contention on single directory lock)
    def cargo_shared_cmd(wt):
        return measure_command(["cargo", "build", "--offline"], wt[1], env=env_cargo_shared)

    print("Running Cargo Shared Target 30x Warm Swarm (Target Lock Contention)...")
    cargo_shared_swarm = run_parallel_builds(cargo_shared_cmd, worktrees)
    cargo_shared_swarm["disk_mb"] = round(get_dir_size_mb(shared_target_dir), 1)
    print(f"  -> Cargo Shared 30x Warm: {cargo_shared_swarm['wall_sec']}s | CPU: {cargo_shared_swarm['cpu_percent']}% | Disk: {cargo_shared_swarm['disk_mb']}MB | Pass: {cargo_shared_swarm['success_count']}/30")

    # 4D. Cargo Bare Warm Swarm (Each builds from scratch into local target)
    for _, wt_dir in worktrees:
        c_dir = wt_dir / "target"
        if c_dir.exists():
            shutil.rmtree(c_dir)

    def cargo_bare_cmd(wt):
        return measure_command(["cargo", "build", "--offline"], wt[1], env=env_cargo_bare)

    print("Running Cargo Bare 30x Swarm (30x Full Rebuilds)...")
    cargo_bare_swarm = run_parallel_builds(cargo_bare_cmd, worktrees)
    cargo_bare_swarm_disk = sum(get_dir_size_mb(w[1] / "target") for w in worktrees)
    cargo_bare_swarm["disk_mb"] = round(cargo_bare_swarm_disk, 1)
    print(f"  -> Cargo Bare 30x Swarm: {cargo_bare_swarm['wall_sec']}s | CPU: {cargo_bare_swarm['cpu_percent']}% | Disk: {cargo_bare_swarm_disk:.1f}MB | Pass: {cargo_bare_swarm['success_count']}/30")

    # Summary record
    best_baseline_cold = min(cargo_bare_cold["wall_sec"], cargo_shared_cold["wall_sec"], sccache_cold["wall_sec"])
    best_baseline_noop = min(cargo_bare_noop["wall_sec"], cargo_shared_noop["wall_sec"], sccache_noop["wall_sec"])
    best_baseline_wt2 = min(cargo_bare_wt2["wall_sec"], cargo_shared_wt2["wall_sec"], sccache_wt2["wall_sec"])
    best_baseline_swarm = min(cargo_bare_swarm["wall_sec"], cargo_shared_swarm["wall_sec"], sccache_swarm_warm["wall_sec"])

    summary = {
        "project": project_name,
        "single_cold": {
            "mitts": mitts_cold,
            "cargo_bare": cargo_bare_cold,
            "cargo_shared": cargo_shared_cold,
            "sccache": sccache_cold,
            "speedup_vs_best_baseline": round(best_baseline_cold / max(mitts_cold["wall_sec"], 0.001), 2),
            "speedup_vs_cargo_bare": round(cargo_bare_cold["wall_sec"] / max(mitts_cold["wall_sec"], 0.001), 2)
        },
        "single_noop": {
            "mitts": mitts_noop,
            "cargo_bare": cargo_bare_noop,
            "cargo_shared": cargo_shared_noop,
            "sccache": sccache_noop,
            "speedup_vs_best_baseline": round(best_baseline_noop / max(mitts_noop["wall_sec"], 0.0001), 2),
            "speedup_vs_cargo_bare": round(cargo_bare_noop["wall_sec"] / max(mitts_noop["wall_sec"], 0.0001), 2)
        },
        "cross_worktree_wt2": {
            "mitts": mitts_wt2,
            "cargo_bare": cargo_bare_wt2,
            "cargo_shared": cargo_shared_wt2,
            "sccache": sccache_wt2,
            "speedup_vs_best_baseline": round(best_baseline_wt2 / max(mitts_wt2["wall_sec"], 0.001), 2),
            "speedup_vs_cargo_bare": round(cargo_bare_wt2["wall_sec"] / max(mitts_wt2["wall_sec"], 0.001), 2)
        },
        "swarm_30x": {
            "mitts": mitts_swarm_warm,
            "cargo_bare": cargo_bare_swarm,
            "cargo_shared": cargo_shared_swarm,
            "sccache": sccache_swarm_warm,
            "speedup_vs_best_baseline": round(best_baseline_swarm / max(mitts_swarm_warm["wall_sec"], 0.001), 2),
            "speedup_vs_cargo_bare": round(cargo_bare_swarm["wall_sec"] / max(mitts_swarm_warm["wall_sec"], 0.001), 2),
            "disk_savings_vs_cargo_bare": round(cargo_bare_swarm_disk / max(mitts_swarm_disk, 0.001), 2)
        }
    }
    return summary


def main():
    projects = ["ripgrep", "serde", "tokio", "clap", "ring"]
    if len(sys.argv) > 1:
        projects = sys.argv[1:]

    all_results = {}
    for p in projects:
        p_dir = PROJECTS_DIR / p
        wt_dir = BENCH_ROOT / f"worktrees_{p}"
        if not p_dir.exists():
            print(f"Skipping {p}: directory not found at {p_dir}")
            continue
        try:
            res = benchmark_project(p, p_dir, wt_dir)
            all_results[p] = res
        except Exception as e:
            print(f"Failed benchmarking {p}: {e}")
            import traceback
            traceback.print_exc()

    BENCHMARK_RESULTS_DIR.mkdir(parents=True, exist_ok=True)
    out_file = BENCHMARK_RESULTS_DIR / "results_matrix_defensible.json"
    with open(out_file, "w") as f:
        json.dump(all_results, f, indent=2)

    print(f"\nAll benchmark results saved to {out_file}")


if __name__ == "__main__":
    main()
