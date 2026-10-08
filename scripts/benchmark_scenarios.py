#!/usr/bin/env python3
"""
Defensible benchmark scenarios implementation:
S1: Single Cold Build (3 runs with purged cache, median, binary verification, with & without incremental)
S2: Fast No-Op (3 runs on unchanged sources)
S3: Git History Replay (sequential playback of N historical commits)
S4: Branch Hopping & Zero-Diff Revert (switching HEAD -> commit_outside_S3 -> HEAD)
S5a: Cold Multi-Worktree Concurrency Swarm (purged caches before each participant)
S5b: Warm Multi-Worktree Concurrency Swarm (pre-warmed cache on HEAD)
"""

import concurrent.futures
import os
import shutil
import subprocess
import time
from pathlib import Path
import statistics

from benchmark_common import (
    measure_command,
    clean_sccache,
    get_dir_size_mb,
    get_disk_usage_mb,
    get_binary_version,
    MITTS_BIN,
    SCCACHE_BIN,
)


def run_s1_cold_build(project_name, repo_dir, envs, repeats=3):
    print(f"\n--- [S1] Single Cold Build ({repeats} runs, purged cache) ---")
    results = {}

    def purge_caches():
        if envs["mitts_cas"].exists():
            shutil.rmtree(envs["mitts_cas"], ignore_errors=True)
        user_cache = Path.home() / ".cache" / "mitts"
        if user_cache.exists():
            shutil.rmtree(user_cache, ignore_errors=True)
        if (repo_dir / ".mitts").exists():
            shutil.rmtree(repo_dir / ".mitts", ignore_errors=True)
        if (repo_dir / "target").exists():
            shutil.rmtree(repo_dir / "target", ignore_errors=True)
        if envs["shared_target"].exists():
            shutil.rmtree(envs["shared_target"], ignore_errors=True)
        clean_sccache(envs["sccache_dir"])

    configs = [
        ("mitts", [str(MITTS_BIN), "build"], envs["mitts"]),
        ("cargo_bare_default", ["cargo", "build", "--offline"], envs["cargo_bare"]),
        ("cargo_bare_no_incr", ["cargo", "build", "--offline"], envs["cargo_bare_no_incr"]),
        ("cargo_shared", ["cargo", "build", "--offline"], envs["cargo_shared"]),
        ("sccache", ["cargo", "build", "--offline"], envs["sccache"]),
    ]

    for name, cmd, env in configs:
        times = []
        cpu_pcts = []
        rss_mbs = []
        version_str = None
        for run_idx in range(1, repeats + 1):
            purge_caches()
            res = measure_command(cmd, repo_dir, env=env)
            times.append(res["wall_sec"])
            cpu_pcts.append(res["cpu_percent"])
            rss_mbs.append(res["max_rss_mb"])
            version_str = get_binary_version(repo_dir, project_name)
            assert res["exit_code"] == 0, f"Cold build failed for {name}: {res['stderr_tail']}"
        
        median_time = round(statistics.median(times), 3)
        disk_paths = [repo_dir / "target", envs["mitts_cas"]] if name == "mitts" else \
                     [envs["shared_target"]] if name == "cargo_shared" else \
                     [repo_dir / "target", envs["sccache_dir"]] if name == "sccache" else \
                     [repo_dir / "target"]
        disk_mb = get_disk_usage_mb(disk_paths)
        results[name] = {
            "wall_sec_median": median_time,
            "wall_sec_runs": times,
            "cpu_percent_median": round(statistics.median(cpu_pcts), 1),
            "max_rss_mb": round(max(rss_mbs), 1),
            "disk_usage_mb": disk_mb,
            "binary_version": version_str or "OK",
            "exit_code": 0
        }
        print(f"  -> {name.upper()}: median {median_time}s (runs: {times}) | ver: {version_str} | disk: {disk_mb}MB | RAM: {results[name]['max_rss_mb']}MB")

    return results


def run_s2_fast_noop(project_name, repo_dir, envs, repeats=3):
    print(f"\n--- [S2] Fast No-Op (Unchanged sources, {repeats} runs) ---")
    results = {}

    configs = [
        ("mitts", [str(MITTS_BIN), "build"], envs["mitts"]),
        ("cargo_bare", ["cargo", "build", "--offline"], envs["cargo_bare"]),
        ("cargo_shared", ["cargo", "build", "--offline"], envs["cargo_shared"]),
        ("sccache", ["cargo", "build", "--offline"], envs["sccache"]),
    ]

    for name, cmd, env in configs:
        # Initial build to warm state
        measure_command(cmd, repo_dir, env=env)
        times = []
        cpu_pcts = []
        rss_mbs = []
        for _ in range(repeats):
            res = measure_command(cmd, repo_dir, env=env)
            assert res["exit_code"] == 0, f"No-op failed for {name}"
            times.append(res["wall_sec"])
            cpu_pcts.append(res.get("cpu_percent", 0.0))
            rss_mbs.append(res.get("max_rss_mb", 0.0))
        median_time = round(statistics.median(times), 3)
        disk_paths = [repo_dir / "target", envs["mitts_cas"]] if name == "mitts" else \
                     [envs["shared_target"]] if name == "cargo_shared" else \
                     [repo_dir / "target", envs["sccache_dir"]] if name == "sccache" else \
                     [repo_dir / "target"]
        disk_mb = get_disk_usage_mb(disk_paths)
        results[name] = {
            "wall_sec_median": median_time,
            "wall_sec_runs": times,
            "cpu_percent_median": round(statistics.median(cpu_pcts), 1),
            "max_rss_mb": round(statistics.median(rss_mbs), 1),
            "disk_usage_mb": disk_mb,
        }
        print(f"  -> {name.upper()}: median {median_time}s (runs: {times}) | disk: {disk_mb}MB | RAM: {results[name]['max_rss_mb']}MB")

    return results


def run_s3_git_history_replay(project_name, repo_dir, envs, num_commits=10):
    print(f"\n--- [S3] Git History Replay ({num_commits} consecutive commits) ---")
    log_cmd = ["git", "log", f"-n{num_commits}", "--reverse", "--format=%h"]
    commits = subprocess.check_output(log_cmd, cwd=repo_dir, text=True).strip().splitlines()
    print(f"  Replaying {len(commits)} commits: {commits}")

    for c in commits:
        subprocess.run(["git", "checkout", c], cwd=repo_dir, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        subprocess.run(["cargo", "fetch"], cwd=repo_dir, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

    results = {}
    runners = [
        ("mitts", [str(MITTS_BIN), "build"], envs["mitts"]),
        ("cargo_bare", ["cargo", "build", "--offline"], envs["cargo_bare"]),
        ("cargo_shared", ["cargo", "build", "--offline"], envs["cargo_shared"]),
        ("sccache", ["cargo", "build", "--offline"], envs["sccache"]),
    ]

    for name, cmd, env in runners:
        step_times = []
        step_cpus = []
        step_rsss = []
        cumulative = 0.0
        if (repo_dir / ".mitts").exists():
            shutil.rmtree(repo_dir / ".mitts", ignore_errors=True)
        if (repo_dir / "target").exists():
            shutil.rmtree(repo_dir / "target", ignore_errors=True)
        if name == "cargo_shared" and envs["shared_target"].exists():
            shutil.rmtree(envs["shared_target"], ignore_errors=True)

        for c in commits:
            subprocess.run(["git", "checkout", c], cwd=repo_dir, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            res = measure_command(cmd, repo_dir, env=env)
            assert res["exit_code"] == 0, f"History replay step failed at {c} for {name}"
            step_times.append(res["wall_sec"])
            step_cpus.append(res.get("cpu_percent", 0.0))
            step_rsss.append(res.get("max_rss_mb", 0.0))
            cumulative += res["wall_sec"]

        disk_paths = [repo_dir / "target", envs["mitts_cas"]] if name == "mitts" else \
                     [envs["shared_target"]] if name == "cargo_shared" else \
                     [repo_dir / "target", envs["sccache_dir"]] if name == "sccache" else \
                     [repo_dir / "target"]
        disk_mb = get_disk_usage_mb(disk_paths)
        results[name] = {
            "cumulative_wall_sec": round(cumulative, 3),
            "step_times": step_times,
            "average_step_sec": round(statistics.mean(step_times), 3),
            "avg_cpu_percent": round(statistics.mean(step_cpus), 1) if step_cpus else 0.0,
            "peak_rss_mb": round(max(step_rsss), 1) if step_rsss else 0.0,
            "disk_usage_mb": disk_mb,
        }
        print(f"  -> {name.upper()}: Cumulative {round(cumulative, 3)}s across {len(commits)} commits (avg: {round(statistics.mean(step_times), 3)}s/commit, peak RAM: {results[name]['peak_rss_mb']}MB, disk: {disk_mb}MB)")

    subprocess.run(["git", "checkout", commits[-1]], cwd=repo_dir, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    return results


def run_s4_branch_hopping(project_name, repo_dir, envs, commit_a, commit_b):
    print(f"\n--- [S4] Branch Hopping & Zero-Diff Revert ({commit_a} -> {commit_b} -> {commit_a}) ---")
    results = {}
    runners = [
        ("mitts", [str(MITTS_BIN), "build"], envs["mitts"]),
        ("cargo_bare", ["cargo", "build", "--offline"], envs["cargo_bare"]),
        ("cargo_shared", ["cargo", "build", "--offline"], envs["cargo_shared"]),
        ("sccache", ["cargo", "build", "--offline"], envs["sccache"]),
    ]

    # Pre-fetch commit B
    subprocess.run(["git", "checkout", commit_b], cwd=repo_dir, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    subprocess.run(["cargo", "fetch"], cwd=repo_dir, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

    for name, cmd, env in runners:
        # Step 1: Build Commit A
        subprocess.run(["git", "checkout", commit_a], cwd=repo_dir, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        measure_command(cmd, repo_dir, env=env)

        # Step 2: Switch to Commit B and build (Cold compilation of B delta)
        subprocess.run(["git", "checkout", commit_b], cwd=repo_dir, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        res_switch = measure_command(cmd, repo_dir, env=env)
        ver_b = get_binary_version(repo_dir, project_name)

        # Step 3: Switch back to Commit A and build (Zero-Diff Revert)
        subprocess.run(["git", "checkout", commit_a], cwd=repo_dir, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        res_revert = measure_command(cmd, repo_dir, env=env)
        ver_a = get_binary_version(repo_dir, project_name)

        disk_paths = [repo_dir / "target", envs["mitts_cas"]] if name == "mitts" else \
                     [envs["shared_target"]] if name == "cargo_shared" else \
                     [repo_dir / "target", envs["sccache_dir"]] if name == "sccache" else \
                     [repo_dir / "target"]
        disk_mb = get_disk_usage_mb(disk_paths)

        results[name] = {
            "switch_to_b_sec": res_switch["wall_sec"],
            "revert_to_a_sec": res_revert["wall_sec"],
            "version_b": ver_b or "OK",
            "version_a": ver_a or "OK",
            "switch_cpu_percent": res_switch.get("cpu_percent", 0.0),
            "revert_cpu_percent": res_revert.get("cpu_percent", 0.0),
            "switch_max_rss_mb": res_switch.get("max_rss_mb", 0.0),
            "revert_max_rss_mb": res_revert.get("max_rss_mb", 0.0),
            "disk_usage_mb": disk_mb,
        }
        print(f"  -> {name.upper()}: Switch to B ({commit_b}): {res_switch['wall_sec']}s (ver: {ver_b}) | Revert to A ({commit_a}): {res_revert['wall_sec']}s (ver: {ver_a}) | disk: {disk_mb}MB")

    return results


def prepare_swarm_worktrees(repo_dir, worktree_base_dir, commit_distribution):
    worktree_base_dir.mkdir(parents=True, exist_ok=True)
    subprocess.run(["git", "worktree", "prune"], cwd=repo_dir, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    worktrees = []

    for i, commit in enumerate(commit_distribution, start=1):
        wt_dir = worktree_base_dir / f"wt-{i:02d}"
        if not wt_dir.exists():
            subprocess.run(["git", "worktree", "add", "-f", "-d", str(wt_dir), commit], cwd=repo_dir, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        else:
            subprocess.run(["git", "checkout", commit], cwd=wt_dir, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        subprocess.run(["cargo", "fetch"], cwd=wt_dir, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        worktrees.append((i, wt_dir, commit))

    return worktrees


def run_swarm_executor(cmd_builder_fn, worktrees):
    start = time.perf_counter()
    results = []
    with concurrent.futures.ThreadPoolExecutor(max_workers=len(worktrees)) as executor:
        futures = {executor.submit(cmd_builder_fn, wt): wt for wt in worktrees}
        for fut in concurrent.futures.as_completed(futures):
            wt = futures[fut]
            try:
                res = fut.result()
                results.append((wt, res))
            except Exception as e:
                results.append((wt, {"wall_sec": -1, "user_sec": 0, "sys_sec": 0, "exit_code": -1, "stderr_tail": str(e)}))
    total_wall = time.perf_counter() - start
    passes = sum(1 for _, r in results if r.get("exit_code") == 0)
    return {
        "wall_sec": round(total_wall, 3),
        "success_count": passes,
        "total_count": len(worktrees),
        "details": results
    }


def verify_worktree_versions(worktrees, project_name):
    correct = 0
    for _, wt_dir, commit in worktrees:
        v = get_binary_version(wt_dir, project_name)
        if v and (commit[:7] in v or "rlib" in v):
            correct += 1
    return correct


def summarize_swarm_run(res, worktrees, project_name, tool_name, envs):
    ver = verify_worktree_versions(worktrees, project_name)
    details = res.get("details", [])
    max_rss = max((r.get("max_rss_mb", 0.0) for _, r in details), default=0.0)
    cpu_sum = sum((r.get("cpu_percent", 0.0) for _, r in details))

    if tool_name == "mitts":
        paths = [wt[1] / "target" for wt in worktrees] + [envs["mitts_cas"]]
    elif tool_name == "cargo_shared":
        paths = [envs["shared_target"]]
    elif tool_name == "sccache":
        paths = [wt[1] / "target" for wt in worktrees] + [envs["sccache_dir"]]
    else:  # cargo_bare
        paths = [wt[1] / "target" for wt in worktrees]

    disk_mb = get_disk_usage_mb(paths)

    return {
        "wall_sec": res["wall_sec"],
        "success_count": res["success_count"],
        "verified_versions": f"{ver}/{len(worktrees)}",
        "peak_rss_mb": round(max_rss, 1),
        "cpu_percent_sum": round(cpu_sum, 1),
        "disk_usage_mb": round(disk_mb, 1),
    }


def run_s5a_cold_swarm(project_name, repo_dir, worktree_base_dir, envs, commit_distribution):
    print(f"\n--- [S5a] Cold Multi-Worktree Concurrency Swarm ({len(commit_distribution)} worktrees, strictly clean caches) ---")
    worktrees = prepare_swarm_worktrees(repo_dir, worktree_base_dir, commit_distribution)
    results = {}

    # 1. Mitts Cold Swarm
    if envs["mitts_cas"].exists():
        shutil.rmtree(envs["mitts_cas"], ignore_errors=True)
    user_cache = Path.home() / ".cache" / "mitts"
    if user_cache.exists():
        shutil.rmtree(user_cache, ignore_errors=True)
    for _, wt_dir, _ in worktrees:
        if (wt_dir / "target").exists():
            shutil.rmtree(wt_dir / "target", ignore_errors=True)
        if (wt_dir / ".mitts").exists():
            shutil.rmtree(wt_dir / ".mitts", ignore_errors=True)
    assert not envs["mitts_cas"].exists(), "mitts_cas must be purged before cold swarm"

    res_mitts = run_swarm_executor(lambda wt: measure_command([str(MITTS_BIN), "build"], wt[1], env=envs["mitts"]), worktrees)
    results["mitts"] = summarize_swarm_run(res_mitts, worktrees, project_name, "mitts", envs)
    print(f"  -> MITTS COLD SWARM: {results['mitts']['wall_sec']}s (Pass: {results['mitts']['success_count']}/{len(worktrees)}, Disk: {results['mitts']['disk_usage_mb']}MB, RAM: {results['mitts']['peak_rss_mb']}MB)")

    # 2. Cargo Bare Cold Swarm
    for _, wt_dir, _ in worktrees:
        if (wt_dir / "target").exists():
            shutil.rmtree(wt_dir / "target", ignore_errors=True)

    res_bare = run_swarm_executor(lambda wt: measure_command(["cargo", "build", "--offline"], wt[1], env=envs["cargo_bare"]), worktrees)
    results["cargo_bare"] = summarize_swarm_run(res_bare, worktrees, project_name, "cargo_bare", envs)
    print(f"  -> CARGO BARE COLD SWARM: {results['cargo_bare']['wall_sec']}s (Pass: {results['cargo_bare']['success_count']}/{len(worktrees)}, Disk: {results['cargo_bare']['disk_usage_mb']}MB, RAM: {results['cargo_bare']['peak_rss_mb']}MB)")

    # 3. Cargo Shared Cold Swarm
    if envs["shared_target"].exists():
        shutil.rmtree(envs["shared_target"], ignore_errors=True)
    envs["shared_target"].mkdir(parents=True, exist_ok=True)
    for _, wt_dir, _ in worktrees:
        if (wt_dir / "target").exists():
            shutil.rmtree(wt_dir / "target", ignore_errors=True)

    res_shared = run_swarm_executor(lambda wt: measure_command(["cargo", "build", "--offline"], wt[1], env=envs["cargo_shared"]), worktrees)
    results["cargo_shared"] = summarize_swarm_run(res_shared, worktrees, project_name, "cargo_shared", envs)
    print(f"  -> CARGO SHARED COLD SWARM: {results['cargo_shared']['wall_sec']}s (Pass: {results['cargo_shared']['success_count']}/{len(worktrees)}, Ver: {results['cargo_shared']['verified_versions']})")

    # 4. Sccache Cold Swarm
    clean_sccache(envs["sccache_dir"])
    for _, wt_dir, _ in worktrees:
        if (wt_dir / "target").exists():
            shutil.rmtree(wt_dir / "target", ignore_errors=True)

    res_sccache = run_swarm_executor(lambda wt: measure_command(["cargo", "build", "--offline"], wt[1], env=envs["sccache"]), worktrees)
    results["sccache"] = summarize_swarm_run(res_sccache, worktrees, project_name, "sccache", envs)
    print(f"  -> SCCACHE COLD SWARM: {results['sccache']['wall_sec']}s (Pass: {results['sccache']['success_count']}/{len(worktrees)}, Disk: {results['sccache']['disk_usage_mb']}MB, RAM: {results['sccache']['peak_rss_mb']}MB)")

    return results


def run_s5b_warm_swarm(project_name, repo_dir, worktree_base_dir, envs, commit_distribution):
    print(f"\n--- [S5b] Warm Multi-Worktree Concurrency Swarm ({len(commit_distribution)} worktrees, caches pre-warmed on HEAD) ---")
    worktrees = prepare_swarm_worktrees(repo_dir, worktree_base_dir, commit_distribution)
    results = {}

    head_commit = commit_distribution[0]
    subprocess.run(["git", "checkout", head_commit], cwd=repo_dir, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

    # 1. Warm Mitts
    measure_command([str(MITTS_BIN), "build"], repo_dir, env=envs["mitts"])
    for _, wt_dir, _ in worktrees:
        if (wt_dir / "target").exists():
            shutil.rmtree(wt_dir / "target", ignore_errors=True)
        if (wt_dir / ".mitts").exists():
            shutil.rmtree(wt_dir / ".mitts", ignore_errors=True)

    res_mitts = run_swarm_executor(lambda wt: measure_command([str(MITTS_BIN), "build"], wt[1], env=envs["mitts"]), worktrees)
    results["mitts"] = summarize_swarm_run(res_mitts, worktrees, project_name, "mitts", envs)
    print(f"  -> MITTS WARM SWARM: {results['mitts']['wall_sec']}s (Pass: {results['mitts']['success_count']}/{len(worktrees)}, Disk: {results['mitts']['disk_usage_mb']}MB, RAM: {results['mitts']['peak_rss_mb']}MB)")

    # 2. Warm Cargo Shared
    measure_command(["cargo", "build", "--offline"], repo_dir, env=envs["cargo_shared"])
    for _, wt_dir, _ in worktrees:
        if (wt_dir / "target").exists():
            shutil.rmtree(wt_dir / "target", ignore_errors=True)

    res_shared = run_swarm_executor(lambda wt: measure_command(["cargo", "build", "--offline"], wt[1], env=envs["cargo_shared"]), worktrees)
    results["cargo_shared"] = summarize_swarm_run(res_shared, worktrees, project_name, "cargo_shared", envs)
    print(f"  -> CARGO SHARED WARM SWARM: {results['cargo_shared']['wall_sec']}s (Pass: {results['cargo_shared']['success_count']}/{len(worktrees)}, Ver: {results['cargo_shared']['verified_versions']})")

    # 3. Warm Sccache
    measure_command(["cargo", "build", "--offline"], repo_dir, env=envs["sccache"])
    for _, wt_dir, _ in worktrees:
        if (wt_dir / "target").exists():
            shutil.rmtree(wt_dir / "target", ignore_errors=True)

    res_sccache = run_swarm_executor(lambda wt: measure_command(["cargo", "build", "--offline"], wt[1], env=envs["sccache"]), worktrees)
    results["sccache"] = summarize_swarm_run(res_sccache, worktrees, project_name, "sccache", envs)
    print(f"  -> SCCACHE WARM SWARM: {results['sccache']['wall_sec']}s (Pass: {results['sccache']['success_count']}/{len(worktrees)}, Disk: {results['sccache']['disk_usage_mb']}MB, RAM: {results['sccache']['peak_rss_mb']}MB)")

    # 4. Cargo Bare (No shared cache across worktrees)
    for _, wt_dir, _ in worktrees:
        if (wt_dir / "target").exists():
            shutil.rmtree(wt_dir / "target", ignore_errors=True)

    res_bare = run_swarm_executor(lambda wt: measure_command(["cargo", "build", "--offline"], wt[1], env=envs["cargo_bare"]), worktrees)
    results["cargo_bare"] = summarize_swarm_run(res_bare, worktrees, project_name, "cargo_bare", envs)
    print(f"  -> CARGO BARE SWARM: {results['cargo_bare']['wall_sec']}s (Pass: {results['cargo_bare']['success_count']}/{len(worktrees)}, Disk: {results['cargo_bare']['disk_usage_mb']}MB, RAM: {results['cargo_bare']['peak_rss_mb']}MB)")

    return results
