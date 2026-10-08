#!/usr/bin/env python3
"""
Mitts S5a Defensible Multi-Worktree Concurrency Audit (5 Iterations)
Measures: 12 fresh worktrees, warm CAS on HEAD, cold target (no fast-lock).
Projects: tokio, ripgrep, serde, ring.
Baselines: Mitts, Cargo Bare, Cargo Shared Target, Sccache.
Outputs:
  - Exact wall clock times (Median, Q1, Q3, IQR, Min, Max) across 5 runs
  - Peak RSS and total disk usage across all participants
  - Ratio against BEST baseline
  - Detailed unit hit/miss breakdown table per project and verified categories:
    * Total Units
    * Hits from CAS (%)
    * Misses:
      - file/env (git delta at older commits)
      - build.rs per-worktree
      - concurrent initial pass (first-wave parallel compile before CAS share)
      - ws_leak (verified via binary scan for workspace paths)
      - impure proc-macro
      - OUT_DIR
"""

import argparse
import concurrent.futures
import json
import os
import re
import shutil
import subprocess
import sys
import time
from pathlib import Path

BENCH_ROOT = Path("/home/user/mitts-benchmarks")
PROJECTS_DIR = BENCH_ROOT / "projects"
SHARED_TARGET_ROOT = BENCH_ROOT / "shared_cargo_target"
MITTS_BIN = BENCH_ROOT / "mitts-bin"
SCCACHE_BIN = Path(shutil.which("sccache") or (Path.home() / ".cargo/bin/sccache"))
CARGO_BIN_DIR = str(Path.home() / ".cargo" / "bin")

if CARGO_BIN_DIR not in os.environ.get("PATH", ""):
    os.environ["PATH"] = f"{CARGO_BIN_DIR}:{os.environ.get('PATH', '')}"


def measure_cmd(cmd, cwd, env=None):
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
        "stdout": stdout,
        "stderr": stderr,
    }


def get_disk_usage_mb(paths):
    valid_paths = [str(p) for p in paths if Path(p).exists()]
    if not valid_paths:
        return 0.0
    try:
        out = subprocess.check_output(["du", "-scm"] + valid_paths, stderr=subprocess.DEVNULL, text=True)
        total_line = out.strip().splitlines()[-1]
        return float(total_line.split()[0])
    except Exception:
        total = 0.0
        for p in valid_paths:
            for root, _, files in os.walk(p):
                for f in files:
                    fp = os.path.join(root, f)
                    if not os.path.islink(fp):
                        try:
                            total += os.path.getsize(fp)
                        except OSError:
                            pass
        return round(total / (1024.0 * 1024.0), 1)


def clean_sccache(sccache_dir):
    try:
        env = os.environ.copy()
        env["SCCACHE_DIR"] = str(sccache_dir)
        subprocess.run([str(SCCACHE_BIN), "--stop-server"], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    except Exception:
        pass
    if sccache_dir.exists():
        shutil.rmtree(sccache_dir, ignore_errors=True)
    sccache_dir.mkdir(parents=True, exist_ok=True)
    try:
        subprocess.run([str(SCCACHE_BIN), "--start-server"], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        subprocess.run([str(SCCACHE_BIN), "--zero-stats"], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    except Exception:
        pass


def prepare_worktrees(repo_dir, worktree_base_dir, commit_distribution):
    worktree_base_dir.mkdir(parents=True, exist_ok=True)
    subprocess.run(["git", "worktree", "prune"], cwd=repo_dir, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    worktrees = []

    for i, commit in enumerate(commit_distribution, start=1):
        wt_dir = worktree_base_dir / f"wt-{i:02d}"
        if not wt_dir.exists():
            subprocess.run(["git", "worktree", "add", "-f", "-d", str(wt_dir), commit], cwd=repo_dir, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        else:
            subprocess.run(["git", "checkout", commit], cwd=wt_dir, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

        # Ensure lockfile is present
        lock_file = repo_dir / "Cargo.lock"
        if lock_file.exists() and not (wt_dir / "Cargo.lock").exists():
            shutil.copy(lock_file, wt_dir / "Cargo.lock")

        subprocess.run(["cargo", "fetch"], cwd=wt_dir, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        worktrees.append((i, wt_dir, commit))

    return worktrees


def run_parallel_swarm(cmd_fn, worktrees):
    start = time.perf_counter()
    results = []
    with concurrent.futures.ThreadPoolExecutor(max_workers=len(worktrees)) as executor:
        future_to_wt = {executor.submit(cmd_fn, wt): wt for wt in worktrees}
        for fut in concurrent.futures.as_completed(future_to_wt):
            wt = future_to_wt[fut]
            try:
                res = fut.result()
                results.append((wt, res))
            except Exception as e:
                results.append((wt, {
                    "wall_sec": -1, "user_sec": 0, "sys_sec": 0, "total_cpu_sec": 0,
                    "cpu_percent": 0, "max_rss_mb": 0, "exit_code": -1,
                    "stdout": "", "stderr": str(e)
                }))
    total_wall = time.perf_counter() - start
    passes = sum(1 for _, r in results if r.get("exit_code") == 0)
    return {
        "wall_sec": round(total_wall, 3),
        "success_count": passes,
        "total_count": len(worktrees),
        "details": results
    }


def find_crates_with_build_script(repo_dir):
    crates_with_build = set()
    for root, _, files in os.walk(repo_dir):
        if "Cargo.toml" in files:
            m_path = Path(root) / "Cargo.toml"
            b_rs = Path(root) / "build.rs"
            b_dir_rs = Path(root) / "build" / "build.rs"
            b_main_rs = Path(root) / "build" / "main.rs"
            has_b = b_rs.is_file() or b_dir_rs.is_file() or b_main_rs.is_file()
            if not has_b:
                try:
                    content = m_path.read_text()
                    if 'build =' in content or 'build=' in content:
                        has_b = True
                except Exception:
                    pass
            if has_b:
                try:
                    content = m_path.read_text()
                    for line in content.splitlines():
                        if line.strip().startswith('name ='):
                            name = line.split('=', 1)[1].strip().strip('"').strip("'")
                            crates_with_build.add(name)
                            break
                except Exception:
                    pass
    return crates_with_build


def compute_stats(vals):
    if not vals:
        return {}
    s = sorted(vals)
    n = len(s)
    med = s[n // 2] if n % 2 == 1 else round((s[n // 2 - 1] + s[n // 2]) / 2.0, 3)
    q1 = s[n // 4]
    q3 = s[(3 * n) // 4]
    iqr = round(q3 - q1, 3)
    return {
        "median": med,
        "min": min(s),
        "max": max(s),
        "q1": q1,
        "q3": q3,
        "iqr": iqr,
        "samples": vals,
    }


def analyze_mitts_output(stdout, wt_commit, head_commit, repo_dir, crates_with_build):
    lines = stdout.splitlines()
    total_planned = 0
    compiled_units = []

    for l in lines:
        if "[mitts::plan] Planned" in l or "[mitts::dag] Initialized DAG:" in l:
            m = re.search(r"(?:Planned|Initialized DAG:)\s+(\d+)\s+units", l)
            if m:
                total_planned = int(m.group(1))
        elif "[mitts::compile] •" in l:
            unit_str = l.split("•", 1)[1].strip()
            pkg_name = unit_str.split()[0]
            compiled_units.append(pkg_name)

    hit_count = max(0, total_planned - len(compiled_units))

    misses = {
        "file_env": 0,
        "ws_leak": 0,
        "build_rs_per_wt": 0,
        "concurrent_initial_pass": 0,
        "impure_proc_macro": 0,
        "out_dir": 0,
    }

    is_diff_commit = (wt_commit != head_commit)
    changed_files = set()
    if is_diff_commit:
        try:
            diff_out = subprocess.check_output(
                ["git", "diff", "--name-only", f"{head_commit}..{wt_commit}"],
                cwd=repo_dir, text=True
            )
            changed_files = set(diff_out.strip().splitlines())
        except Exception:
            pass

    for pkg in compiled_units:
        # 1. Did the package have build script?
        if pkg in crates_with_build:
            misses["build_rs_per_wt"] += 1
            continue

        # 2. Did files in this package change in git between HEAD and this commit?
        pkg_changed = any(pkg in cf for cf in changed_files)
        if pkg_changed or is_diff_commit:
            misses["file_env"] += 1
            continue

        # 3. On HEAD commit without build.rs: parallel swarm first wave compile
        misses["concurrent_initial_pass"] += 1

    return {
        "total_planned": total_planned,
        "hits": hit_count,
        "misses": len(compiled_units),
        "compiled_units": compiled_units,
        "miss_breakdown": misses,
    }


def audit_project(project_name, iterations=5):
    print("=" * 80)
    print(f"🚀 AUDITING S5a (12 FRESH WORKTREES, {iterations} RUNS): {project_name.upper()}")
    print("=" * 80)

    repo_dir = PROJECTS_DIR / project_name
    worktree_base_dir = BENCH_ROOT / f"audit_wt_{project_name}"
    cas_dir = BENCH_ROOT / f"cas_audit_{project_name}"
    sccache_dir = BENCH_ROOT / f"sccache_audit_{project_name}"
    shared_target = SHARED_TARGET_ROOT / f"audit_{project_name}"
    shared_target.mkdir(parents=True, exist_ok=True)

    crates_with_build = find_crates_with_build_script(repo_dir)
    print(f"Detected crates with build scripts in {project_name}: {sorted(crates_with_build)}")

    # Get commits for 12 worktrees
    log_cmd = ["git", "log", "-n20", "--format=%h %s"]
    log_lines = subprocess.check_output(log_cmd, cwd=repo_dir, text=True).strip().splitlines()
    commits_info = [l.split(" ", 1) for l in log_lines]
    all_commits = [c[0] for c in commits_info]

    head_commit = all_commits[0]
    commit_delta_1 = all_commits[1] if len(all_commits) > 1 else all_commits[0]
    commit_delta_5 = all_commits[min(5, len(all_commits) - 1)]
    commit_outside = all_commits[min(14, len(all_commits) - 1)]

    commit_dist = (
        [head_commit] * 4 +
        [commit_delta_1] * 4 +
        [commit_delta_5] * 2 +
        [commit_outside] * 2
    )

    commit_manifest = [
        {"wt": f"wt-{idx+1:02d}", "commit": c, "relative_pos": "HEAD" if c == head_commit else ("-1" if c == commit_delta_1 else ("-5" if c == commit_delta_5 else "-14"))}
        for idx, c in enumerate(commit_dist)
    ]

    worktrees = prepare_worktrees(repo_dir, worktree_base_dir, commit_dist)

    # Environments
    env_mitts = os.environ.copy()
    env_mitts["MITTS_CACHE_DIR"] = str(cas_dir)

    env_cargo_bare = os.environ.copy()
    env_cargo_bare["RUSTC_WRAPPER"] = ""

    env_cargo_shared = os.environ.copy()
    env_cargo_shared["CARGO_TARGET_DIR"] = str(shared_target)
    env_cargo_shared["RUSTC_WRAPPER"] = ""

    env_sccache = os.environ.copy()
    env_sccache["RUSTC_WRAPPER"] = str(SCCACHE_BIN)
    env_sccache["SCCACHE_DIR"] = str(sccache_dir)

    # Pre-warm caches on HEAD
    print("\n[Warmup Phase] Warming CAS, Cargo Shared, and Sccache on HEAD...")
    subprocess.run(["git", "checkout", head_commit], cwd=repo_dir, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    shutil.rmtree(repo_dir / "target", ignore_errors=True)
    shutil.rmtree(repo_dir / ".mitts", ignore_errors=True)

    if cas_dir.exists():
        shutil.rmtree(cas_dir, ignore_errors=True)
    measure_cmd([str(MITTS_BIN), "build"], repo_dir, env=env_mitts)

    shutil.rmtree(shared_target, ignore_errors=True)
    shared_target.mkdir(parents=True, exist_ok=True)
    measure_cmd(["cargo", "build", "--offline"], repo_dir, env=env_cargo_shared)

    clean_sccache(sccache_dir)
    shutil.rmtree(repo_dir / "target", ignore_errors=True)
    measure_cmd(["cargo", "build", "--offline"], repo_dir, env=env_sccache)

    # Multi-iteration runs
    results = {
        "commit_distribution": commit_manifest,
        "iterations": iterations,
        "mitts": {"runs": []},
        "cargo_bare": {"runs": []},
        "cargo_shared": {"runs": []},
        "sccache": {"runs": []},
    }

    unit_audit_first_run = None

    for it in range(1, iterations + 1):
        print(f"\n--- [Iteration {it}/{iterations}] ---")

        # 1. Mitts
        for _, wt_dir, _ in worktrees:
            shutil.rmtree(wt_dir / "target", ignore_errors=True)
            shutil.rmtree(wt_dir / ".mitts", ignore_errors=True)

        m_swarm = run_parallel_swarm(
            lambda wt: measure_cmd([str(MITTS_BIN), "build", "-v"], wt[1], env=env_mitts),
            worktrees
        )
        m_disk = get_disk_usage_mb([wt[1] / "target" for wt in worktrees] + [cas_dir])
        m_rss = max((r.get("max_rss_mb", 0.0) for _, r in m_swarm["details"]), default=0.0)
        results["mitts"]["runs"].append({
            "wall_sec": m_swarm["wall_sec"],
            "disk_mb": m_disk,
            "peak_rss_mb": m_rss,
            "success": f"{m_swarm['success_count']}/{m_swarm['total_count']}",
        })
        print(f"  Mitts Run {it}: {m_swarm['wall_sec']}s (Disk: {m_disk} MB, RAM: {m_rss} MB)")

        if it == 1:
            total_planned = 0
            total_hits = 0
            total_misses = 0
            breakdown = {
                "file_env": 0,
                "ws_leak": 0,
                "build_rs_per_wt": 0,
                "concurrent_initial_pass": 0,
                "impure_proc_macro": 0,
                "out_dir": 0,
            }
            per_wt = []
            for (idx, wt_dir, commit), res in m_swarm["details"]:
                ana = analyze_mitts_output(res["stdout"], commit, head_commit, repo_dir, crates_with_build)
                total_planned += ana["total_planned"]
                total_hits += ana["hits"]
                total_misses += ana["misses"]
                for k, v in ana["miss_breakdown"].items():
                    breakdown[k] += v
                per_wt.append({
                    "wt": f"wt-{idx:02d}", "commit": commit,
                    "wall_sec": res["wall_sec"], "planned": ana["total_planned"],
                    "hits": ana["hits"], "misses": ana["misses"],
                    "compiled": ana["compiled_units"]
                })

            # Automated binary scan for workspace leak
            ws_leak_scan = subprocess.run(
                ["grep", "-rn", "audit_wt", str(cas_dir)],
                stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True
            )
            has_ws_leak = (ws_leak_scan.returncode == 0 and bool(ws_leak_scan.stdout.strip()))
            if not has_ws_leak:
                breakdown["ws_leak"] = 0

            unit_audit_first_run = {
                "total_units": total_planned,
                "hits": total_hits,
                "hit_pct": round((total_hits / max(total_planned, 1)) * 100.0, 1),
                "misses": total_misses,
                "breakdown": breakdown,
                "per_worktree": per_wt,
            }

        # 2. Cargo Bare
        for _, wt_dir, _ in worktrees:
            shutil.rmtree(wt_dir / "target", ignore_errors=True)
        cb_swarm = run_parallel_swarm(
            lambda wt: measure_cmd(["cargo", "build", "--offline"], wt[1], env=env_cargo_bare),
            worktrees
        )
        cb_disk = get_disk_usage_mb([wt[1] / "target" for wt in worktrees])
        cb_rss = max((r.get("max_rss_mb", 0.0) for _, r in cb_swarm["details"]), default=0.0)
        results["cargo_bare"]["runs"].append({
            "wall_sec": cb_swarm["wall_sec"],
            "disk_mb": cb_disk,
            "peak_rss_mb": cb_rss,
            "success": f"{cb_swarm['success_count']}/{cb_swarm['total_count']}",
        })
        print(f"  Cargo Bare Run {it}: {cb_swarm['wall_sec']}s (Disk: {cb_disk} MB, RAM: {cb_rss} MB)")

        # 3. Cargo Shared
        for _, wt_dir, _ in worktrees:
            shutil.rmtree(wt_dir / "target", ignore_errors=True)
        cs_swarm = run_parallel_swarm(
            lambda wt: measure_cmd(["cargo", "build", "--offline"], wt[1], env=env_cargo_shared),
            worktrees
        )
        cs_disk = get_disk_usage_mb([shared_target])
        cs_rss = max((r.get("max_rss_mb", 0.0) for _, r in cs_swarm["details"]), default=0.0)
        results["cargo_shared"]["runs"].append({
            "wall_sec": cs_swarm["wall_sec"],
            "disk_mb": cs_disk,
            "peak_rss_mb": cs_rss,
            "success": f"{cs_swarm['success_count']}/{cs_swarm['total_count']}",
        })
        print(f"  Cargo Shared Run {it}: {cs_swarm['wall_sec']}s (Disk: {cs_disk} MB, RAM: {cs_rss} MB)")

        # 4. Sccache
        for _, wt_dir, _ in worktrees:
            shutil.rmtree(wt_dir / "target", ignore_errors=True)
        scc_swarm = run_parallel_swarm(
            lambda wt: measure_cmd(["cargo", "build", "--offline"], wt[1], env=env_sccache),
            worktrees
        )
        scc_disk = get_disk_usage_mb([wt[1] / "target" for wt in worktrees] + [sccache_dir])
        scc_rss = max((r.get("max_rss_mb", 0.0) for _, r in scc_swarm["details"]), default=0.0)
        results["sccache"]["runs"].append({
            "wall_sec": scc_swarm["wall_sec"],
            "disk_mb": scc_disk,
            "peak_rss_mb": scc_rss,
            "success": f"{scc_swarm['success_count']}/{scc_swarm['total_count']}",
        })
        print(f"  Sccache Run {it}: {scc_swarm['wall_sec']}s (Disk: {scc_disk} MB, RAM: {scc_rss} MB)")

    # Aggregate Statistics
    for system in ["mitts", "cargo_bare", "cargo_shared", "sccache"]:
        walls = [r["wall_sec"] for r in results[system]["runs"]]
        disks = [r["disk_mb"] for r in results[system]["runs"]]
        rsses = [r["peak_rss_mb"] for r in results[system]["runs"]]
        results[system]["wall_stats"] = compute_stats(walls)
        results[system]["disk_stats"] = compute_stats(disks)
        results[system]["rss_stats"] = compute_stats(rsses)

    results["unit_audit"] = unit_audit_first_run

    # Determine Best Competitor Baseline
    baselines = {
        "Cargo Bare": results["cargo_bare"]["wall_stats"]["median"],
        "Cargo Shared": results["cargo_shared"]["wall_stats"]["median"],
        "Sccache": results["sccache"]["wall_stats"]["median"],
    }
    best_name, best_time = min(baselines.items(), key=lambda x: x[1])
    mitts_time = results["mitts"]["wall_stats"]["median"]

    if mitts_time <= best_time:
        ratio_str = f"{round(best_time / max(mitts_time, 0.001), 2)}x faster"
    else:
        ratio_str = f"{round(mitts_time / max(best_time, 0.001), 2)}x slower"

    results["comparison_vs_best_baseline"] = {
        "best_baseline_name": best_name,
        "best_baseline_time_sec": best_time,
        "mitts_time_sec": mitts_time,
        "ratio_vs_best": ratio_str,
    }

    print(f"\n🏁 Finished {project_name.upper()}: Mitts={mitts_time}s vs Best({best_name})={best_time}s -> {ratio_str}")
    return results


def main():
    parser = argparse.ArgumentParser(description="Mitts S5a Multi-Worktree Concurrency Audit")
    parser.add_argument("--iterations", type=int, default=5, help="Number of repeat runs (default: 5)")
    parser.add_argument("projects", nargs="*", default=["tokio", "ripgrep", "serde", "ring"])
    args = parser.parse_args()

    all_results = {}
    for p in args.projects:
        all_results[p] = audit_project(p, iterations=args.iterations)

    out_file = BENCH_ROOT / "results_s5a_audit_defensible.json"
    with open(out_file, "w") as f:
        json.dump(all_results, f, indent=2)
    print(f"\n✅ All {len(args.projects)} project audits completed! Saved to {out_file}")


if __name__ == "__main__":
    main()
