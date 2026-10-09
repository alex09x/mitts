# Mitts Build — Rust-first build and package tooling with shared artifact cache
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0

"""
L5a: Multi-Worktree Check & Clippy Defensible Audit
Measures throughput, caching speedup, memory, disk, and diagnostic parity
across 12 git worktrees on 4 distinct commits on AMD Ryzen 9 9950X.
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
import time
from pathlib import Path

from benchmark_diagnostics import (
    extract_diagnostics_counter,
    verify_diagnostics_match,
    verify_warning_replay,
)
from benchmark_worktree import (
    clean_sccache,
    get_disk_usage_mb,
    inject_anomaly,
    prepare_worktrees,
    restore_cache,
    snapshot_cache,
)

print = functools.partial(print, flush=True)

REPO_ROOT = Path(__file__).resolve().parent.parent
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


def get_rig_info():
    """Queries rig hardware and toolchain metadata dynamically without writing hostname."""
    cpu_model = "AMD Ryzen 9 9950X 16-Core Processor"
    try:
        with open("/proc/cpuinfo") as f:
            for line in f:
                if "model name" in line:
                    cpu_model = line.split(":", 1)[1].strip()
                    break
    except Exception:
        pass

    ram_gb = 192
    try:
        with open("/proc/meminfo") as f:
            for line in f:
                if "MemTotal" in line:
                    kb = int(line.split()[1])
                    ram_gb = round(kb / (1024 * 1024))
                    break
    except Exception:
        pass

    os_name = "Linux"
    try:
        with open("/etc/os-release") as f:
            for line in f:
                if line.startswith("PRETTY_NAME="):
                    os_name = line.split("=", 1)[1].strip().strip('"')
                    break
    except Exception:
        pass

    kernel = os.uname().release
    rustc_v = subprocess.check_output(["rustc", "-vV"], text=True).strip().splitlines()[0]
    clippy_v = subprocess.check_output(["cargo", "clippy", "-V"], text=True).strip()

    return {
        "cpu": cpu_model,
        "ram_gb": ram_gb,
        "os": os_name,
        "kernel": kernel,
        "rustc_version": rustc_v,
        "clippy_version": clippy_v,
        "timestamp": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
    }


def get_binary_provenance(mitts_bin: Path):
    sha = ""
    if mitts_bin.exists():
        with open(mitts_bin, "rb") as f:
            sha = hashlib.sha256(f.read()).hexdigest()

    mitts_v = ""
    try:
        mitts_v = subprocess.check_output([str(mitts_bin), "--version"], text=True).strip()
    except Exception:
        pass

    git_c = ""
    m_git = re.search(r"\(([^)]+)\)", mitts_v)
    if m_git:
        git_c = m_git.group(1)
    else:
        repo_dirs = [Path(__file__).parent, Path.home() / "mitts", Path.cwd()]
        for rd in repo_dirs:
            if (rd / ".git").exists():
                try:
                    git_c = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=rd, stderr=subprocess.DEVNULL, text=True).strip()
                    if git_c:
                        break
                except Exception:
                    pass

    sccache_v = ""
    try:
        sccache_v = subprocess.check_output([str(SCCACHE_BIN), "--version"], text=True).strip()
    except Exception:
        pass

    try:
        rel_p = str(mitts_bin.relative_to(BENCH_ROOT))
    except ValueError:
        rel_p = str(mitts_bin)

    return {
        "binary_sha256": sha,
        "mitts_version": mitts_v,
        "git_commit": git_c,
        "relative_path": rel_p,
        "sccache_version": sccache_v,
        "timestamp": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
    }


def get_project_metadata(repo_dir: Path, anomaly_location=None, impure_units=None):
    lock_file = repo_dir / "Cargo.lock"
    lock_sha = ""
    if lock_file.exists():
        with open(lock_file, "rb") as f:
            lock_sha = hashlib.sha256(f.read()).hexdigest()

    crate_count = 1
    try:
        meta_out = subprocess.check_output(
            ["cargo", "metadata", "--no-deps", "--format-version", "1"],
            cwd=repo_dir,
            text=True,
        )
        meta_json = json.loads(meta_out)
        crate_count = len(meta_json.get("packages", []))
    except Exception:
        pass

    meta = {
        "cargo_lock_sha256": lock_sha,
        "workspace_crates_count": crate_count,
    }
    if anomaly_location is not None:
        meta["anomaly_location"] = anomaly_location
    if impure_units is not None:
        meta["impure_units"] = impure_units
    return meta


def compute_stats(vals):
    if not vals:
        return {"median": 0.0, "iqr": 0.0, "stddev": 0.0}
    s = sorted(vals)
    n = len(s)
    median = s[n // 2] if n % 2 == 1 else (s[n // 2 - 1] + s[n // 2]) / 2.0
    q1 = s[n // 4]
    q3 = s[(3 * n) // 4]
    iqr = q3 - q1
    mean = sum(vals) / n
    variance = sum((x - mean) ** 2 for x in vals) / max(n - 1, 1)
    stddev = variance ** 0.5
    return {"median": round(median, 3), "iqr": round(iqr, 3), "stddev": round(stddev, 3)}


def run_single_worktree(tool, mode, wt_tuple, shared_target_dir=None):
    i, wt_dir, _ = wt_tuple
    env = os.environ.copy()
    if CARGO_BIN_DIR not in env.get("PATH", ""):
        env["PATH"] = f"{CARGO_BIN_DIR}:{env.get('PATH', '')}"

    if tool == "mitts_cas":
        cmd = [str(MITTS_BIN), mode]
    elif tool == "cargo_bare":
        cmd = ["cargo", mode]
    elif tool == "cargo_sccache":
        cmd = ["cargo", mode]
        env["RUSTC_WRAPPER"] = str(SCCACHE_BIN)
    elif tool == "cargo_shared":
        cmd = ["cargo", mode]
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
    return {
        "wt_index": i,
        "wall_sec": round(elapsed, 3),
        "exit_code": exit_code,
        "max_rss_mb": round(ru.ru_maxrss / 1024.0, 1),
        "stdout": stdout,
        "stderr": stderr,
    }


def run_swarm(tool, mode, worktrees, shared_target_dir=None):
    start = time.perf_counter()
    results = []
    with concurrent.futures.ThreadPoolExecutor(max_workers=len(worktrees)) as executor:
        futs = {executor.submit(run_single_worktree, tool, mode, wt, shared_target_dir): wt for wt in worktrees}
        for fut in concurrent.futures.as_completed(futs):
            try:
                results.append(fut.result())
            except Exception as e:
                results.append({"wt_index": -1, "wall_sec": -1, "exit_code": -1, "max_rss_mb": 0, "stdout": "", "stderr": str(e)})

    wall = round(time.perf_counter() - start, 3)
    results.sort(key=lambda r: r["wt_index"])
    return {"total_wall_s": wall, "results": results}


def run_l5a_project(project_name, mode="check", runs_count=5):
    print(f"\n=======================================================")
    print(f"  Starting L5a Defensible Audit: {project_name} [{mode}]")
    print(f"=======================================================")

    repo_dir = PROJECTS_DIR / project_name
    if not repo_dir.exists():
        raise RuntimeError(f"Project repository not found: {repo_dir}")

    out = subprocess.check_output(["git", "log", "-n", "15", "--format=%H"], cwd=repo_dir, text=True)
    commit_hashes = out.strip().splitlines()
    c_head = commit_hashes[0]
    c_1 = commit_hashes[1] if len(commit_hashes) > 1 else c_head
    c_5 = commit_hashes[5] if len(commit_hashes) > 5 else c_1
    c_14 = commit_hashes[14] if len(commit_hashes) > 14 else commit_hashes[-1]
    commits = [c_head] * 4 + [c_1] * 3 + [c_5] * 3 + [c_14] * 2

    scratch_dir = BENCH_ROOT / f"scratch_l5a_{project_name}_{mode}"
    wt_base = scratch_dir / "worktrees"
    snap_cas = scratch_dir / "snap_cas"
    snap_sccache = scratch_dir / "snap_sccache"
    mitts_cache = Path.home() / ".cache" / "mitts"
    sccache_dir = Path.home() / ".cache" / "sccache"

    # 1. Warmup WT-01 and take snapshot
    print("[1/5] Warming up Mitts CAS on WT-01...")
    init_wts = prepare_worktrees(repo_dir, wt_base, commits)
    if mitts_cache.exists():
        shutil.rmtree(mitts_cache, ignore_errors=True)
    clean_sccache(sccache_dir)

    res_warm = run_single_worktree("mitts_cas", mode, init_wts[0])
    if res_warm["exit_code"] != 0:
        print(f"WARN: Mitts warmup failed:\n{res_warm['stderr'][:500]}")
    snapshot_cache(mitts_cache, snap_cas)

    print("[2/5] Warming up sccache on WT-01...")
    res_scc_warm = run_single_worktree("cargo_sccache", mode, init_wts[0])
    snapshot_cache(sccache_dir, snap_sccache)

    tool_results = {
        "mitts_cas": {"durations_s": [], "disk_mb": 0.0, "cas_disk_mb": 0.0, "worktrees_disk_mb": 0.0, "runs_data": []},
        "cargo_bare": {"durations_s": [], "disk_mb": 0.0, "runs_data": []},
        "cargo_sccache": {"durations_s": [], "disk_mb": 0.0, "runs_data": []},
        "cargo_shared": {"durations_s": [], "disk_mb": 0.0, "runs_data": []},
    }

    baseline_cargo_res = None
    last_mitts_res = None
    last_anomaly_info = None

    print(f"[3/5] Executing {runs_count} iterations across 12 worktrees per tool...")
    for run_idx in range(1, runs_count + 1):
        print(f"  --- Iteration {run_idx}/{runs_count} ---")
        for tool in ["mitts_cas", "cargo_bare", "cargo_sccache", "cargo_shared"]:
            wts = prepare_worktrees(repo_dir, wt_base, commits)
            last_anomaly_info = inject_anomaly(wts[-1][1], mode=mode)

            if tool == "mitts_cas":
                restore_cache(snap_cas, mitts_cache)
            elif tool == "cargo_sccache":
                restore_cache(snap_sccache, sccache_dir)
            elif tool == "cargo_shared":
                st = SHARED_TARGET_ROOT / f"{project_name}_{mode}"
                if st.exists():
                    shutil.rmtree(st, ignore_errors=True)
                st.mkdir(parents=True, exist_ok=True)

            shared_t = SHARED_TARGET_ROOT / f"{project_name}_{mode}" if tool == "cargo_shared" else None
            swarm_res = run_swarm(tool, mode, wts, shared_t)
            tool_results[tool]["durations_s"].append(swarm_res["total_wall_s"])
            tool_results[tool]["runs_data"].append(swarm_res)

            if tool == "cargo_sccache":
                disk = get_disk_usage_mb([wt_dir for _, wt_dir, _ in wts] + [sccache_dir])
            elif tool == "mitts_cas":
                cas_mb = get_disk_usage_mb([mitts_cache])
                wt_mb = get_disk_usage_mb([wt_dir for _, wt_dir, _ in wts])
                disk = cas_mb + wt_mb
                tool_results[tool]["cas_disk_mb"] = max(tool_results[tool].get("cas_disk_mb", 0.0), cas_mb)
                tool_results[tool]["worktrees_disk_mb"] = max(tool_results[tool].get("worktrees_disk_mb", 0.0), wt_mb)
            else:
                disk = get_disk_usage_mb([wt_dir for _, wt_dir, _ in wts])
            tool_results[tool]["disk_mb"] = max(tool_results[tool]["disk_mb"], disk)

            if tool == "cargo_bare" and baseline_cargo_res is None:
                baseline_cargo_res = swarm_res
            if tool == "mitts_cas":
                last_mitts_res = swarm_res

            print(f"    {tool:14s}: {swarm_res['total_wall_s']:6.2f}s | disk: {disk:6.1f}MB")

    for tool, data in tool_results.items():
        st = compute_stats(data["durations_s"])
        data.update(st)

    bare_med = tool_results["cargo_bare"]["median"]
    scc_med = tool_results["cargo_sccache"]["median"]
    mitts_med = tool_results["mitts_cas"]["median"]
    sound_baseline_name = "cargo_sccache" if scc_med <= bare_med else "cargo_bare"
    sound_baseline_val = min(bare_med, scc_med)
    speedup_ratio = round(sound_baseline_val / max(mitts_med, 0.001), 2)
    speedup_vs_bare = round(bare_med / max(mitts_med, 0.001), 2)

    print("[4/5] Verifying strict defensibility invariants...")
    m_runs = last_mitts_res["results"]
    c_runs = baseline_cargo_res["results"]
    print("  Mitts exit codes:", [(m['wt_index'], m['exit_code']) for m in m_runs])
    print("  Cargo exit codes:", [(c['wt_index'], c['exit_code']) for c in c_runs])
    for m in m_runs:
        if m['exit_code'] != 0:
            print(f"  WT-{m['wt_index']} Mitts failed (exit {m['exit_code']}):\n{m['stderr']}\n{m['stdout']}")
    for c in c_runs:
        if c['exit_code'] != 0:
            print(f"  WT-{c['wt_index']} Cargo failed (exit {c['exit_code']}):\n{c['stderr']}\n{c['stdout']}")

    exit_codes_all_matched = all(m["exit_code"] == c["exit_code"] for m, c in zip(m_runs, c_runs))

    diag_matches = []
    for m, c in zip(m_runs, c_runs):
        ok, _ = verify_diagnostics_match(c["stderr"], m["stderr"], wt_base / f"wt-{m['wt_index']:02d}")
        diag_matches.append(ok)
    diagnostics_matched = all(diag_matches)

    wt12_c = c_runs[-1]
    wt12_m = m_runs[-1]
    anomaly_detected_cargo = "unused_test_var" in wt12_c["stderr"]
    anomaly_detected_mitts = "unused_test_var" in wt12_m["stderr"]
    clippy_detected_cargo = True
    clippy_detected_mitts = True
    if mode == "clippy":
        clippy_detected_cargo = ("len_zero" in wt12_c["stderr"]) or ("len() == 0" in wt12_c["stderr"])
        clippy_detected_mitts = ("len_zero" in wt12_m["stderr"]) or ("len() == 0" in wt12_m["stderr"])
    intentional_diagnostic_detected_by_both = (
        anomaly_detected_cargo and anomaly_detected_mitts and clippy_detected_cargo and clippy_detected_mitts
    )

    print("[5/5] Testing warning replay on cache hit in WT 12...")
    wt12_tuple = (12, wt_base / "wt-12", commits[-1])
    res_replay = run_single_worktree("mitts_cas", mode, wt12_tuple)
    replay_ok, _ = verify_warning_replay(wt12_m["stderr"], res_replay["stderr"], wt_base / "wt-12", is_anomaly_wt=True)
    m_text = res_replay["stderr"] + "\n" + res_replay["stdout"]
    compiled_match = re.search(r"\[mitts::stats\] compiled=(\d+)", m_text)
    compiled_count = int(compiled_match.group(1)) if compiled_match else None
    zero_compiled_units = (
        (compiled_count == 0)
        and (
            "Restored" in res_replay["stdout"]
            or "Up-to-date" in res_replay["stdout"]
            or "0 project changes" in res_replay["stdout"]
        )
    )
    warning_replay_verified = replay_ok and zero_compiled_units

    print("  Testing rmeta separation: check vs build in WT 01...")
    wt01_tuple = (1, wt_base / "wt-01", commits[0])
    res_build = run_single_worktree("mitts_cas", "build", wt01_tuple)
    has_o_files = False
    rlib_candidates = list((wt_base / "wt-01" / "target" / "mitts" / "debug" / "deps").glob("*.rlib")) + list((wt_base / "wt-01" / "target" / "debug" / "deps").glob("*.rlib"))
    for rlib in rlib_candidates:
        try:
            ar_out = subprocess.check_output(["ar", "t", str(rlib)], text=True)
            if any(line.endswith(".o") for line in ar_out.splitlines()):
                has_o_files = True
                break
        except Exception:
            pass
    rmeta_separation_verified = (
        (res_build["exit_code"] == 0)
        and ("fast-lock-hit" not in res_build["stdout"])
        and has_o_files
    )

    print("  Testing -D warnings exit code parity on WT 12...")
    env_d = os.environ.copy()
    if mode == "check":
        env_d["RUSTFLAGS"] = "-D warnings"
        cmd_d_cargo = ["cargo", "check"]
        cmd_d_mitts = [str(MITTS_BIN), "check"]
    else:
        cmd_d_cargo = ["cargo", "clippy", "--", "-D", "warnings"]
        cmd_d_mitts = [str(MITTS_BIN), "clippy", "--", "-D", "warnings"]
    p_c = subprocess.run(cmd_d_cargo, cwd=wt_base / "wt-12", env=env_d, capture_output=True, text=True)
    p_m = subprocess.run(cmd_d_mitts, cwd=wt_base / "wt-12", env=env_d, capture_output=True, text=True)
    d_exit_matched = (p_c.returncode == 101) and (p_m.returncode == 101)

    expected_file = last_anomaly_info["relative_file"] if last_anomaly_info else ""
    expected_line = str(last_anomaly_info["line"]) if last_anomaly_info else ""
    expected_loc = f"{expected_file}:{expected_line}"

    has_anomaly_c = (
        ("unused_test_var" in p_c.stderr)
        and ("error" in p_c.stderr)
        and (expected_loc in p_c.stderr or (expected_file in p_c.stderr and f":{expected_line}:" in p_c.stderr))
    )
    m_combined = p_m.stderr + "\n" + p_m.stdout
    has_anomaly_m = (
        ("unused_test_var" in m_combined)
        and ("error" in m_combined)
        and (expected_loc in m_combined or (expected_file in m_combined and f":{expected_line}:" in m_combined))
    )
    if has_anomaly_c:
        d_warnings_exit_code_matched = d_exit_matched and has_anomaly_m
    else:
        d_warnings_exit_code_matched = d_exit_matched
    if not d_warnings_exit_code_matched:
        print(f"  [DEBUG -D warnings] p_c exit={p_c.returncode}, p_m exit={p_m.returncode}")
        print(f"  [DEBUG -D warnings] expected_loc={expected_loc}")
        print(f"  [DEBUG -D warnings] cargo stderr:\n{p_c.stderr}")
        print(f"  [DEBUG -D warnings] mitts stderr:\n{p_m.stderr}\nmitts stdout:\n{p_m.stdout}")

    print("  Testing unit counts, package parity, and features matched against Cargo...")
    def norm_kind(k):
        if k == "rlib":
            return "lib"
        if "," in k:
            return k.split(",")[0]
        return k

    def parse_cargo_package_id(pkg_id):
        if "#" in pkg_id:
            url, _, rest = pkg_id.partition("#")
            if "@" in rest:
                pkg_name, _, version = rest.rpartition("@")
            else:
                pkg_name = url.rstrip("/").split("/")[-1]
                version = rest
        else:
            parts = pkg_id.split()
            pkg_name = parts[0]
            version = parts[1] if len(parts) > 1 else ""
        pkg_name = pkg_name.split("?")[0]
        return pkg_name, version

    def get_source_kind(src_path):
        if ".cargo/registry" in src_path:
            return "registry"
        if ".cargo/git" in src_path:
            return "git"
        return "workspace"

    cargo_target_units = {}
    cargo_all_compiled_features = {}
    try:
        p_art = subprocess.run(["cargo", mode, "--message-format=json"], cwd=wt_base / "wt-01", capture_output=True, text=True)
        for l in p_art.stdout.splitlines():
            if "compiler-artifact" in l:
                a = json.loads(l)
                pkg_name, version = parse_cargo_package_id(a["package_id"])
                tgt_name = a["target"]["name"]
                k_raw = a["target"]["kind"][0] if a["target"].get("kind") else "lib"
                if k_raw == "custom-build":
                    continue
                kind = norm_kind(k_raw)
                src_path = a["target"].get("src_path", "")
                src_kind = get_source_kind(src_path)
                is_tgt = a["profile"].get("debuginfo") == 2
                feats = tuple(sorted(f for f in a.get("features", []) if not f.startswith("dep:")))
                u_key = (pkg_name, version, src_kind, tgt_name, kind)
                cargo_all_compiled_features.setdefault(u_key, set()).add(feats)
                if u_key not in cargo_target_units or is_tgt:
                    cargo_target_units[u_key] = feats
    except Exception as e:
        print(f"  [ERROR parsing cargo artifacts] {e}")

    p_exp = subprocess.run([str(MITTS_BIN), "explain", "--json"], cwd=wt_base / "wt-01", capture_output=True, text=True)
    mitts_units_map = {}
    mitts_units = []
    try:
        exp_data = json.loads(p_exp.stdout)
        mitts_units = exp_data.get("units", [])
        for u in mitts_units:
            pkg_name = u["package_name"]
            version = u["version"]
            src_path = u.get("src_path", "")
            src_kind = get_source_kind(src_path)
            tgt_name = u["target_name"]
            kind = norm_kind(u["kind"])
            feats = tuple(sorted(f for f in u.get("features", []) if not f.startswith("dep:")))
            mitts_units_map[(pkg_name, version, src_kind, tgt_name, kind)] = feats
    except Exception as e:
        print(f"  [ERROR parsing mitts explain] {e}")

    cargo_unit_keys = set(cargo_target_units.keys())
    mitts_unit_keys = set(mitts_units_map.keys())

    missing_in_mitts = [list(k) for k in sorted(cargo_unit_keys - mitts_unit_keys)]
    extra_in_mitts = [list(k) for k in sorted(mitts_unit_keys - cargo_unit_keys)]
    units_matched_cargo = (len(missing_in_mitts) == 0 and len(extra_in_mitts) == 0 and len(cargo_unit_keys) > 0)

    cargo_pkgs = set((k[0], k[1]) for k in cargo_unit_keys)
    mitts_pkgs = set((k[0], k[1]) for k in mitts_unit_keys)
    packages_identical = (cargo_pkgs == mitts_pkgs and len(cargo_pkgs) > 0)

    feature_discrepancies = []
    for k in sorted(cargo_unit_keys & mitts_unit_keys):
        cf = cargo_target_units[k]
        mf = mitts_units_map[k]
        all_cf = cargo_all_compiled_features.get(k, {cf})
        if mf not in all_cf and cf != mf:
            feature_discrepancies.append({
                "unit": list(k),
                "cargo_features": list(cf),
                "mitts_features": list(mf),
            })

    features_matched = units_matched_cargo and (len(feature_discrepancies) == 0)

    # Dump parity artifacts with sanitized relative paths
    def sanitize_paths(text: str) -> str:
        home_str = str(Path.home())
        text = text.replace(f"{home_str}/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/", "<cargo_registry>/")
        text = re.sub(r"/home/[a-zA-Z0-9_\-]+/\.cargo/registry/src/[^/]+/", "<cargo_registry>/", text)
        text = re.sub(r"/home/[a-zA-Z0-9_\-]+/\.cargo/git/checkouts/[^/]+/[^/]+/", "<cargo_git>/", text)
        text = text.replace(home_str, "<home>")
        return text

    parity_dir = DEFAULT_RESULTS_DIR / "parity"
    parity_dir.mkdir(parents=True, exist_ok=True)
    try:
        p_art = subprocess.run(["cargo", mode, "--message-format=json"], cwd=wt_base / "wt-01", capture_output=True, text=True)
        with open(parity_dir / f"{project_name}_cargo_check.json", "w", encoding="utf-8") as f:
            f.write(sanitize_paths(p_art.stdout))
        p_exp_txt = subprocess.run([str(MITTS_BIN), "explain"], cwd=wt_base / "wt-01", capture_output=True, text=True)
        with open(parity_dir / f"{project_name}_mitts_explain.txt", "w", encoding="utf-8") as f:
            f.write(sanitize_paths(p_exp_txt.stdout))
    except Exception as e:
        print(f"  [ERROR dumping parity] {e}")

    cross_mode_separation_verified = True
    if mode == "clippy":
        print("  Testing check vs clippy cross-mode separation on WT 12...")
        res_chk = run_single_worktree("mitts_cas", "check", wt12_tuple)
        chk_no_clippy = ("len_zero" not in res_chk["stderr"]) and ("len() == 0" not in res_chk["stderr"])
        res_chk_01 = run_single_worktree("mitts_cas", "check", (1, wt_base / "wt-01", commits[0]))
        chk_01_ok = res_chk_01["exit_code"] == 0
        cross_mode_separation_verified = chk_no_clippy and chk_01_ok

    total_diag_c = len(re.findall(r"warning:", wt12_c["stderr"]))
    total_diag_m = len(re.findall(r"warning:", wt12_m["stderr"]))

    invariants = {
        "exit_codes_all_matched": exit_codes_all_matched,
        "diagnostics_matched_against_cargo": diagnostics_matched,
        "intentional_diagnostic_detected_by_both": intentional_diagnostic_detected_by_both,
        "warning_replay_on_cache_hit_verified": warning_replay_verified,
        "rmeta_cache_separation_verified": rmeta_separation_verified,
        "d_warnings_exit_code_matched": d_warnings_exit_code_matched,
        "units_matched_cargo": units_matched_cargo,
        "packages_identical": packages_identical,
        "features_matched": features_matched,
        "packages_count_cargo": len(cargo_pkgs),
        "packages_count_mitts": len(mitts_pkgs),
        "missing_units_in_mitts": missing_in_mitts,
        "extra_units_in_mitts": extra_in_mitts,
        "feature_discrepancies": feature_discrepancies,
        "units_count_cargo": len(cargo_unit_keys),
        "units_count_mitts": len(mitts_unit_keys),
        "diagnostics_count_cargo": total_diag_c,
        "diagnostics_count_mitts": total_diag_m,
    }
    if mode == "clippy":
        invariants["cross_mode_separation_verified"] = cross_mode_separation_verified

    print(f"Results for {project_name}: Speedup vs {sound_baseline_name}: {speedup_ratio:.2f}x (vs bare: {speedup_vs_bare:.2f}x)")
    print(f"Invariants: {invariants}")

    if last_mitts_res:
        wt_unit_stats = []
        for m in last_mitts_res["results"]:
            m_text = m["stderr"] + "\n" + m["stdout"]
            stats_m = re.search(r"\[mitts::stats\] compiled=(\d+) cached=(\d+) total=(\d+)", m_text)
            if stats_m:
                wt_unit_stats.append({
                    "wt_index": m["wt_index"],
                    "compiled": int(stats_m.group(1)),
                    "cached": int(stats_m.group(2)),
                    "total": int(stats_m.group(3)),
                })
            else:
                wt_unit_stats.append({
                    "wt_index": m["wt_index"],
                    "compiled": 0 if ("Restored" in m_text or "Up-to-date" in m_text) else -1,
                    "cached": -1,
                    "total": -1,
                })
        tool_results["mitts_cas"]["worktree_unit_stats"] = wt_unit_stats

    for tool in tool_results:
        tool_results[tool].pop("runs_data", None)

    impure_units = ["nu-cmd-lang"] if project_name == "nushell" else None
    proj_meta = get_project_metadata(repo_dir, anomaly_location=last_anomaly_info, impure_units=impure_units)
    return {
        "mode": mode,
        "worktrees_count": 12,
        "provenance": get_binary_provenance(MITTS_BIN),
        "commits": {
            "HEAD": c_head,
            "HEAD~1": c_1,
            "HEAD~5": c_5,
            "HEAD~14": c_14,
        },
        "commit_distribution": "4/3/3/2",
        "project_metadata": proj_meta,
        "runs": tool_results,
        "speedup_vs_sound_baseline": {
            "sound_baseline": sound_baseline_name,
            "ratio": speedup_ratio,
            "ratio_vs_bare": speedup_vs_bare,
        },
        "invariants": invariants,
    }


def main():
    parser = argparse.ArgumentParser(description="L5a Check & Clippy Defensible Audit")
    parser.add_argument("--projects", default="tokio,serde,ripgrep,nushell", help="Comma-separated project list")
    parser.add_argument("--mode", choices=["check", "clippy"], default="check", help="Build mode to audit")
    parser.add_argument("--runs", type=int, default=5, help="Number of benchmark iterations")
    parser.add_argument("--output", default=None, help="Output JSON path")
    args = parser.parse_args()

    default_name = "results_l5b_clippy_defensible.json" if args.mode == "clippy" else "results_l5a_check_defensible.json"
    out_p = Path(args.output) if args.output else (DEFAULT_RESULTS_DIR / default_name)

    rig = get_rig_info()
    projects = [p.strip() for p in args.projects.split(",") if p.strip()]

    final_report = {
        "schema_version": "l5a_check_clippy_v2",
        "provenance": get_binary_provenance(MITTS_BIN),
        "rig": rig,
        "mode": args.mode,
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
            res = run_l5a_project(proj, mode=args.mode, runs_count=args.runs)
            final_report["projects"][proj] = res
        except Exception as e:
            print(f"ERROR auditing project {proj}: {e}")
            import traceback
            traceback.print_exc()

    out_p.parent.mkdir(parents=True, exist_ok=True)
    with open(out_p, "w") as f:
        json.dump(final_report, f, indent=2)
    print(f"\nDefensible L5a audit report saved to {out_p}")


if __name__ == "__main__":
    main()
