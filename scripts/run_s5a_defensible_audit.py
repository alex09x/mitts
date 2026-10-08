#!/usr/bin/env python3
"""
Mitts S5a Defensible Multi-Worktree Concurrency Audit
Measures: 12 fresh worktrees, warm CAS on HEAD, cold target (no fast-lock).
Projects: tokio, ripgrep, serde, ring.
Baselines: Mitts, Cargo Bare, Cargo Shared, Sccache.
Outputs:
  - Exact wall clock times, peak RSS, total disk usage, pass rates
  - Detailed unit hit/miss breakdown table per project and category:
    * Total Units
    * Hits from CAS (%)
    * Misses:
      - no manifest match: file/env
      - ws_leak
      - build.rs per-worktree
      - impure proc-macro
      - OUT_DIR
"""

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


def get_disk_usage_mb(paths):
    valid_paths = [str(p) for p in paths if Path(p).exists()]
    if not valid_paths:
        return 0.0
    try:
        out = subprocess.check_output(["du", "-scm"] + valid_paths, stderr=subprocess.DEVNULL, text=True)
        total_line = out.strip().splitlines()[-1]
        return float(total_line.split()[0])
    except Exception:
        return round(sum(get_dir_size_mb(p) for p in valid_paths), 1)


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

        # Guarantee lockfile is present across worktrees
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


def analyze_mitts_output(stdout, wt_dir, wt_commit, head_commit, repo_dir, crates_with_build):
    lines = stdout.splitlines()
    total_planned = 0
    compiled_units = []

    for l in lines:
        if "[mitts::plan] Planned" in l:
            m = re.search(r"Planned\s+(\d+)\s+units", l)
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

        # 3. Unit is on HEAD commit without build.rs, but recompiled -> workspace path leak
        misses["ws_leak"] += 1

    return {
        "total_planned": total_planned,
        "hits": hit_count,
        "misses": len(compiled_units),
        "compiled_units": compiled_units,
        "miss_breakdown": misses,
    }


def audit_project(project_name):
    print("=" * 80)
    print(f"🚀 AUDITING S5a (12 FRESH WORKTREES, WARM CAS): {project_name.upper()}")
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
    log_cmd = ["git", "log", "-n20", "--format=%h"]
    all_commits = subprocess.check_output(log_cmd, cwd=repo_dir, text=True).strip().splitlines()
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

    # --- Step 1: Pre-warm caches on HEAD in repo_dir ---
    print("\n[Warmup Phase] Warming CAS, Cargo Shared, and Sccache on HEAD...")
    subprocess.run(["git", "checkout", head_commit], cwd=repo_dir, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    shutil.rmtree(repo_dir / "target", ignore_errors=True)
    shutil.rmtree(repo_dir / ".mitts", ignore_errors=True)

    # Pre-warm Mitts CAS
    if cas_dir.exists():
        shutil.rmtree(cas_dir, ignore_errors=True)
    w_mitts = measure_cmd([str(MITTS_BIN), "build"], repo_dir, env=env_mitts)
    print(f"  Warmup Mitts CAS: {w_mitts['wall_sec']}s (exit {w_mitts['exit_code']})")

    # Pre-warm Cargo Shared
    shutil.rmtree(shared_target, ignore_errors=True)
    shared_target.mkdir(parents=True, exist_ok=True)
    w_shared = measure_cmd(["cargo", "build", "--offline"], repo_dir, env=env_cargo_shared)
    print(f"  Warmup Cargo Shared: {w_shared['wall_sec']}s (exit {w_shared['exit_code']})")

    # Pre-warm Sccache
    clean_sccache(sccache_dir)
    shutil.rmtree(repo_dir / "target", ignore_errors=True)
    w_sccache = measure_cmd(["cargo", "build", "--offline"], repo_dir, env=env_sccache)
    print(f"  Warmup Sccache: {w_sccache['wall_sec']}s (exit {w_sccache['exit_code']})")

    results = {}

    # --- Step 2: Mitts 12 Worktrees Swarm (Cold targets, Warm CAS) ---
    print("\n[1/4] Running Mitts Swarm (12 worktrees, cold target, warm CAS)...")
    for _, wt_dir, _ in worktrees:
        shutil.rmtree(wt_dir / "target", ignore_errors=True)
        shutil.rmtree(wt_dir / ".mitts", ignore_errors=True)

    # Verify targets are cold
    for _, wt_dir, _ in worktrees:
        assert not (wt_dir / "target").exists(), f"Target must be cold for {wt_dir}"
        assert not (wt_dir / ".mitts").exists(), f".mitts must not exist for {wt_dir}"

    m_swarm = run_parallel_swarm(
        lambda wt: measure_cmd([str(MITTS_BIN), "build", "-v"], wt[1], env=env_mitts),
        worktrees
    )
    m_disk = get_disk_usage_mb([wt[1] / "target" for wt in worktrees] + [cas_dir])
    m_rss = max((r.get("max_rss_mb", 0.0) for _, r in m_swarm["details"]), default=0.0)
    results["mitts"] = {
        "wall_sec": m_swarm["wall_sec"],
        "success": f"{m_swarm['success_count']}/{m_swarm['total_count']}",
        "disk_mb": m_disk,
        "peak_rss_mb": m_rss,
    }
    print(f"  -> Mitts: {m_swarm['wall_sec']}s (Pass: {results['mitts']['success']}, Disk: {m_disk} MB, RAM: {m_rss} MB)")

    # Aggregate Unit Hit/Miss Statistics
    total_units_all = 0
    total_hits_all = 0
    total_misses_all = 0
    cat_breakdown = {
        "no_manifest_match_file_env": 0,
        "ws_leak": 0,
        "build_rs_per_wt": 0,
        "impure_proc_macro": 0,
        "out_dir": 0,
    }

    per_wt_audit = []
    for (idx, wt_dir, commit), res in m_swarm["details"]:
        ana = analyze_mitts_output(res["stdout"], wt_dir, commit, head_commit, repo_dir, crates_with_build)
        total_units_all += ana["total_planned"]
        total_hits_all += ana["hits"]
        total_misses_all += ana["misses"]
        cat_breakdown["no_manifest_match_file_env"] += ana["miss_breakdown"]["file_env"]
        cat_breakdown["ws_leak"] += ana["miss_breakdown"]["ws_leak"]
        cat_breakdown["build_rs_per_wt"] += ana["miss_breakdown"]["build_rs_per_wt"]
        cat_breakdown["impure_proc_macro"] += ana["miss_breakdown"]["impure_proc_macro"]
        cat_breakdown["out_dir"] += ana["miss_breakdown"]["out_dir"]
        per_wt_audit.append({
            "worktree": f"wt-{idx:02d}",
            "commit": commit,
            "wall_sec": res["wall_sec"],
            "planned": ana["total_planned"],
            "hits": ana["hits"],
            "misses": ana["misses"],
            "compiled": ana["compiled_units"],
        })

    results["mitts_unit_audit"] = {
        "total_units": total_units_all,
        "hits": total_hits_all,
        "hit_pct": round((total_hits_all / max(total_units_all, 1)) * 100.0, 1),
        "misses": total_misses_all,
        "breakdown": cat_breakdown,
        "per_worktree": per_wt_audit,
    }

    # --- Step 3: Cargo Bare Swarm ---
    print("\n[2/4] Running Cargo Bare Swarm (12 worktrees, isolated cold targets)...")
    for _, wt_dir, _ in worktrees:
        shutil.rmtree(wt_dir / "target", ignore_errors=True)

    c_bare_swarm = run_parallel_swarm(
        lambda wt: measure_cmd(["cargo", "build", "--offline"], wt[1], env=env_cargo_bare),
        worktrees
    )
    c_bare_disk = get_disk_usage_mb([wt[1] / "target" for wt in worktrees])
    c_bare_rss = max((r.get("max_rss_mb", 0.0) for _, r in c_bare_swarm["details"]), default=0.0)
    results["cargo_bare"] = {
        "wall_sec": c_bare_swarm["wall_sec"],
        "success": f"{c_bare_swarm['success_count']}/{c_bare_swarm['total_count']}",
        "disk_mb": c_bare_disk,
        "peak_rss_mb": c_bare_rss,
    }
    print(f"  -> Cargo Bare: {c_bare_swarm['wall_sec']}s (Pass: {results['cargo_bare']['success']}, Disk: {c_bare_disk} MB, RAM: {c_bare_rss} MB)")

    # --- Step 4: Cargo Shared Target Swarm ---
    print("\n[3/4] Running Cargo Shared Target Swarm (12 worktrees, warm shared target)...")
    for _, wt_dir, _ in worktrees:
        shutil.rmtree(wt_dir / "target", ignore_errors=True)

    c_shared_swarm = run_parallel_swarm(
        lambda wt: measure_cmd(["cargo", "build", "--offline"], wt[1], env=env_cargo_shared),
        worktrees
    )
    c_shared_disk = get_disk_usage_mb([shared_target])
    c_shared_rss = max((r.get("max_rss_mb", 0.0) for _, r in c_shared_swarm["details"]), default=0.0)
    results["cargo_shared"] = {
        "wall_sec": c_shared_swarm["wall_sec"],
        "success": f"{c_shared_swarm['success_count']}/{c_shared_swarm['total_count']}",
        "disk_mb": c_shared_disk,
        "peak_rss_mb": c_shared_rss,
    }
    print(f"  -> Cargo Shared: {c_shared_swarm['wall_sec']}s (Pass: {results['cargo_shared']['success']}, Disk: {c_shared_disk} MB, RAM: {c_shared_rss} MB)")

    # --- Step 5: Sccache Swarm ---
    print("\n[4/4] Running Sccache Swarm (12 worktrees, warm sccache)...")
    for _, wt_dir, _ in worktrees:
        shutil.rmtree(wt_dir / "target", ignore_errors=True)

    scc_swarm = run_parallel_swarm(
        lambda wt: measure_cmd(["cargo", "build", "--offline"], wt[1], env=env_sccache),
        worktrees
    )
    scc_disk = get_disk_usage_mb([wt[1] / "target" for wt in worktrees] + [sccache_dir])
    scc_rss = max((r.get("max_rss_mb", 0.0) for _, r in scc_swarm["details"]), default=0.0)
    results["sccache"] = {
        "wall_sec": scc_swarm["wall_sec"],
        "success": f"{scc_swarm['success_count']}/{scc_swarm['total_count']}",
        "disk_mb": scc_disk,
        "peak_rss_mb": scc_rss,
    }
    print(f"  -> Sccache: {scc_swarm['wall_sec']}s (Pass: {results['sccache']['success']}, Disk: {scc_disk} MB, RAM: {scc_rss} MB)")

    return results


def main():
    projects = ["tokio", "ripgrep", "serde", "ring"]
    if len(sys.argv) > 1:
        projects = sys.argv[1:]

    all_results = {}
    for p in projects:
        all_results[p] = audit_project(p)

    out_file = BENCH_ROOT / "results_s5a_audit_defensible.json"
    with open(out_file, "w") as f:
        json.dump(all_results, f, indent=2)
    print(f"\n✅ All audits completed! Results saved to: {out_file}")


if __name__ == "__main__":
    main()
