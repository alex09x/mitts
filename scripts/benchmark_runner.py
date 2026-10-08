#!/usr/bin/env python3
"""
Mitts Benchmark Runner — Per-Project Rigorous Execution
Usage:
  python3 benchmark_runner.py [--project ripgrep] [--commits 10]
"""

import argparse
import json
import os
import subprocess
import sys
from pathlib import Path

from benchmark_common import (
    CARGO_BIN_DIR,
    MITTS_BIN,
    SCCACHE_BIN,
)
from benchmark_scenarios import (
    run_s1_cold_build,
    run_s2_fast_noop,
    run_s3_git_history_replay,
    run_s4_branch_hopping,
    run_s5a_cold_swarm,
    run_s5b_warm_swarm,
)

BENCH_ROOT = Path("/home/user/mitts-benchmarks")
PROJECTS_DIR = BENCH_ROOT / "projects"
SHARED_TARGET_ROOT = BENCH_ROOT / "shared_cargo_target"


def parse_args():
    parser = argparse.ArgumentParser(description="Mitts Defensible Benchmark Runner")
    parser.add_argument("--project", default="ripgrep", help="Target project to benchmark (default: ripgrep)")
    parser.add_argument("--commits", type=int, default=10, help="Number of commits for Git history replay (default: 10)")
    parser.add_argument("--output", default=None, help="Output JSON path")
    return parser.parse_args()


def setup_environments(project_name):
    shared_cas = BENCH_ROOT / f"cas_bench_{project_name}"
    sccache_dir = BENCH_ROOT / f"sccache_bench_{project_name}"
    shared_target_dir = SHARED_TARGET_ROOT / project_name
    shared_target_dir.mkdir(parents=True, exist_ok=True)

    env_cargo_bare = os.environ.copy()
    env_cargo_bare["RUSTC_WRAPPER"] = ""

    env_cargo_bare_no_incr = os.environ.copy()
    env_cargo_bare_no_incr["RUSTC_WRAPPER"] = ""
    env_cargo_bare_no_incr["CARGO_INCREMENTAL"] = "0"

    env_cargo_shared = os.environ.copy()
    env_cargo_shared["CARGO_TARGET_DIR"] = str(shared_target_dir)
    env_cargo_shared["RUSTC_WRAPPER"] = ""

    env_sccache = os.environ.copy()
    env_sccache["RUSTC_WRAPPER"] = str(SCCACHE_BIN)
    env_sccache["SCCACHE_DIR"] = str(sccache_dir)
    env_sccache["CARGO_INCREMENTAL"] = "0"

    env_mitts = os.environ.copy()
    env_mitts["MITTS_CACHE_DIR"] = str(shared_cas)

    return {
        "mitts": env_mitts,
        "cargo_bare": env_cargo_bare,
        "cargo_bare_no_incr": env_cargo_bare_no_incr,
        "cargo_shared": env_cargo_shared,
        "sccache": env_sccache,
        "mitts_cas": shared_cas,
        "sccache_dir": sccache_dir,
        "shared_target": shared_target_dir,
    }


def main():
    args = parse_args()
    project_name = args.project
    repo_dir = PROJECTS_DIR / project_name
    worktree_base_dir = BENCH_ROOT / f"worktrees_{project_name}"

    if not repo_dir.exists():
        print(f"Error: Project directory {repo_dir} does not exist.")
        sys.exit(1)

    print("=" * 80)
    print(f"🎯 STARTING BENCHMARK: {project_name.upper()} on AMD Ryzen 9 9950X (Zen 5)")
    print(f"   Repo: {repo_dir}")
    print(f"   Mitts binary: {MITTS_BIN}")
    print(f"   Sccache binary: {SCCACHE_BIN}")
    print("=" * 80)

    envs = setup_environments(project_name)

    # 1. Inspect Git History (fetch 20 commits so we have commits outside S3)
    log_cmd = ["git", "log", "-n20", "--format=%h"]
    all_commits = subprocess.check_output(log_cmd, cwd=repo_dir, text=True).strip().splitlines()
    head_commit = all_commits[0]
    commit_delta_1 = all_commits[1] if len(all_commits) > 1 else all_commits[0]
    commit_delta_5 = all_commits[min(5, len(all_commits) - 1)]
    # Pick commit outside S3 replay window for S4
    commit_outside_s3 = all_commits[min(14, len(all_commits) - 1)]

    # 2. Run Scenarios
    results = {
        "project": project_name,
        "head_commit": head_commit,
        "commit_outside_s3": commit_outside_s3,
        "s1_cold_build": run_s1_cold_build(project_name, repo_dir, envs, repeats=3),
        "s2_fast_noop": run_s2_fast_noop(project_name, repo_dir, envs, repeats=3),
        "s3_git_history_replay": run_s3_git_history_replay(project_name, repo_dir, envs, num_commits=args.commits),
        "s4_branch_hopping": run_s4_branch_hopping(project_name, repo_dir, envs, head_commit, commit_outside_s3),
    }

    # S5: 12 worktrees distributed across commits
    commit_dist = (
        [head_commit] * 4 +
        [commit_delta_1] * 4 +
        [commit_delta_5] * 2 +
        [commit_outside_s3] * 2
    )
    results["s5a_cold_swarm"] = run_s5a_cold_swarm(
        project_name, repo_dir, worktree_base_dir, envs, commit_dist
    )
    results["s5b_warm_swarm"] = run_s5b_warm_swarm(
        project_name, repo_dir, worktree_base_dir, envs, commit_dist
    )

    # 3. Save JSON output
    out_file = Path(args.output) if args.output else (BENCH_ROOT / f"results_{project_name}_defensible.json")
    with open(out_file, "w") as f:
        json.dump(results, f, indent=2)
    print(f"\n✅ Benchmark completed successfully! Full results saved to: {out_file}")


if __name__ == "__main__":
    main()
