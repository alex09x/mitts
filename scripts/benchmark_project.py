#!/usr/bin/env python3
# Mitts Build — Rust-first build and package tooling with shared artifact cache
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0

"""
Unified Benchmark Runner for Mitts Build (T0 -> T5, Linux XFS/ext4 & macOS APFS).

Executes the full M8 methodology on a single repository (`--repo`) or a corpus
directory (`--projects-dir`), relocating both worktrees (`--work-dir`) and all
shared caches (`--cache-dir`: `mitts_cas`, `sccache`, `cargo_shared_target`,
`cargo_home`) onto the target filesystem device.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
import time
import traceback
from pathlib import Path

from benchmark_common import drain_gate_events, effective_max_busy, measure_cpu_baseline, set_quiet_policy, wait_for_quiet_rig
from benchmark_lib import (
    TOOLS, chmod_rmtree, collect_provenance, copy_repo_tree, ensure_commit_history,
    evaluate_t4_invariants, evaluate_t5_invariants, git_checkout_clean, prepare_commit_locks_and_fetch,
    prepare_swarm_worktrees, purge_mitts_cas, restore_dir, run_timed, setup_environments,
    start_sccache_zero_stats, stop_sccache, tool_cmd, verify_diagnostics_parity,
)
from benchmark_pressure import sync_and_wait_dirty, get_device_io_bytes, get_df_used_mb
from benchmark_stages import measure_swarm_arm, run_stages_t0_t1_t2_t3, run_warmups_and_snapshot
from benchmark_worktree import inject_anomaly
from t5_parsers import (
    compute_stats, inject_test_defect, parse_build_output, parse_mitts_io, parse_test_results, resource_stats,
    select_workspace_targets, timed_resources,
)

# Statuses a resumed run keeps: measured results, noisy or not, and policy skips. Only an
# ERROR (or, on request, NOISY) project is measured again.
RESUME_KEEP_PREFIXES = ("OK", "NOISY", "INVALID_", "SKIPPED_", "EXCLUDED_")
SWARM_KEYS = ("runs", "wall_s", "device_mb_written", "df_used_delta_mb", "disk_mb", "sccache_stats", "loadavg_after", "pressure")


def validate_swarm_commits(src_dir: Path, commits_desc: list[str], swarm_commits: list[str], ctx: dict, extra_args: list[str], res: dict) -> list[str]:
    """Replaces a swarm commit whose tests Cargo itself cannot build with the next older commit
    that can. futures at HEAD~14 carries a `compile_error!` unless every feature is on: all
    four arms failed those worktrees alike and the project read INVALID_T5 while saying nothing
    about the tools. Preparation only, outside every timed window; `cargo check --tests` is
    enough to trip a compile_error! and may use the network for an untracked lockfile."""
    verdict: dict[str, bool] = {}

    def checks(commit: str) -> bool:
        if commit not in verdict:
            git_checkout_clean(src_dir, commit)
            chmod_rmtree(src_dir / "target")
            r = run_timed(["cargo", "check", "--workspace", "--tests"] + extra_args, cwd=src_dir, env=ctx["envs"]["cargo_bare"])
            chmod_rmtree(src_dir / "target")
            verdict[commit] = r["exit_code"] == 0
        return verdict[commit]

    substitutions: dict[str, str] = {}
    for c in list(dict.fromkeys(swarm_commits)):
        if checks(c):
            continue
        start = commits_desc.index(c) + 1 if c in commits_desc else len(commits_desc)
        replacement = next((cand for cand in commits_desc[start:] if checks(cand)), None)
        if replacement is not None:
            substitutions[c] = replacement
            print(f"  [prep] swarm commit {c[:12]} does not build its tests under Cargo; using {replacement[:12]} instead", flush=True)
        else:
            print(f"  [prep] swarm commit {c[:12]} does not build its tests under Cargo and no older commit does either", flush=True)
    if substitutions:
        res["swarm_commit_substitutions"] = {k[:12]: v[:12] for k, v in substitutions.items()}
    return [substitutions.get(c, c) for c in swarm_commits]


def cargo_run_if_fails(subcmd: str, cwd: Path, ctx: dict, extra_args: list[str]) -> dict | None:
    """Runs Cargo's `subcmd` on the same tree Mitts failed on; the run when Cargo fails too, else None."""
    r_c = run_timed(tool_cmd("cargo_bare", subcmd, ctx, extra_args), cwd=cwd, env=ctx["envs"]["cargo_bare"])
    return None if r_c["exit_code"] == 0 else r_c


def cargo_also_fails(res: dict, subcmd: str, cwd: Path, ctx: dict, extra_args: list[str], stage: str, r_mitts: dict) -> bool:
    """When Mitts cannot build a target set, run Cargo on the same tree: a project Cargo cannot
    build either is broken upstream, not a Mitts failure, and is recorded as SKIPPED with both
    outputs instead of ERROR (askama's workspace tests do not compile under either tool)."""
    r_c = cargo_run_if_fails(subcmd, cwd, ctx, extra_args)
    if r_c is None:
        return False
    res["status"] = f"SKIPPED_CARGO_FAILS_{stage}"
    res["skip_detail"] = {
        "stage": stage, "command": subcmd, "mitts_exit_code": r_mitts.get("exit_code"), "mitts_stderr_tail": r_mitts.get("stderr_tail", "")[-3000:],
        "cargo_exit_code": r_c["exit_code"], "cargo_stderr_tail": r_c["stderr_tail"][-3000:],
    }
    print(f"  [{stage.lower()}] cargo fails on the same tree (exit {r_c['exit_code']}): project skipped as broken upstream", flush=True)
    return True


def _swarm_summary(runs: list[dict], wall_key: str, fail_key: str) -> dict:
    dev_vals = [r["device_mb_written"] for r in runs if r["device_mb_written"] is not None]
    rd_vals = [r["device_mb_read"] for r in runs if r["device_mb_read"] is not None]
    return {
        "wall_stats_s": compute_stats([r[wall_key] for r in runs if r[fail_key] == 0]),
        "max_disk_mb": max((r["disk_mb"] for r in runs), default=0.0),
        "median_device_mb_written": compute_stats(dev_vals)["median"] if dev_vals else None,
        "median_device_mb_read": compute_stats(rd_vals)["median"] if rd_vals else None,
        "total_user_stats_s": compute_stats([r["resources"]["total_user_s"] for r in runs]),
        "total_sys_stats_s": compute_stats([r["resources"]["total_sys_s"] for r in runs]),
        "max_wt_rss_mb": max((r["resources"]["max_wt_rss_mb"] for r in runs), default=0.0),
        "units_compiled_median": compute_stats([r["resources"]["units_compiled"] for r in runs])["median"],
        "units_cached_median": compute_stats([r["resources"]["units_cached"] for r in runs])["median"],
    }


def _project_name_from_spec(repo_spec: str) -> str:
    base = os.path.basename(repo_spec.rstrip("/"))
    return base[:-4] if base.endswith(".git") else base


def benchmark_single_project(
    repo_spec: str, work_dir: Path, cache_dir: Path, mitts_bin: Path, sccache_bin: Path,
    max_disk_gb: float, num_commits: int, num_worktrees: int, repeats: int,
    stages: set[str], max_load: float, proj_name: str | None = None, online_fetch: bool = False,
) -> dict:
    max_disk_mb = max_disk_gb * 1024.0
    proj_name = proj_name or _project_name_from_spec(repo_spec)
    proj_work = work_dir / f"proj_{proj_name}.noindex"
    proj_cache = cache_dir / f"cache_{proj_name}.noindex"
    shared_cargo_home = cache_dir / "shared_cargo_home.noindex"
    chmod_rmtree(proj_work)
    chmod_rmtree(proj_cache)
    proj_work.mkdir(parents=True, exist_ok=True)
    proj_cache.mkdir(parents=True, exist_ok=True)

    ctx = setup_environments(proj_work, proj_cache, mitts_bin, sccache_bin, shared_cargo_home=shared_cargo_home)
    src_dir = proj_work / "repo"
    # Per-project arguments, passed to every Cargo and Mitts command alike: the way each
    # project's own CI builds its tests. tokio's tests-build crate is nightly-only, rayon's
    # demo binary uses #![feature], hyper's tests need its `full` feature set.
    extra_args = {
        "tokio": ["--exclude", "tests-build"],
        "rayon": ["--exclude", "rayon-demo"],
        "hyper": ["--features", "full"],
    }.get(proj_name, [])

    res: dict = {
        "project": proj_name, "status": "OK", "status_flags": [],
        "same_device": os.stat(proj_work).st_dev == os.stat(proj_cache).st_dev, "stages": {},
    }
    t_start = time.perf_counter()

    try:
        if repo_spec.startswith(("http://", "https://", "git@", "ssh://")):
            subprocess.run(["git", "clone", "--depth", "25", repo_spec, str(src_dir)], check=True)
            if (src_dir / ".gitmodules").exists():
                subprocess.run(["git", "submodule", "update", "--init", "--recursive", "--force"], cwd=src_dir, check=False)
        else:
            copy_repo_tree(Path(repo_spec).resolve(), src_dir)

        commits_desc = ensure_commit_history(src_dir, max(20, num_commits + 5))
        head_commit, branch_hop_commit = commits_desc[0], commits_desc[min(14, len(commits_desc) - 1)]
        replay_commits = list(reversed(commits_desc[: min(num_commits, len(commits_desc))]))
        pick = lambda idx: commits_desc[idx] if len(commits_desc) > idx else commits_desc[-1]
        c4 = [pick(0), pick(1), pick(5), pick(14)]
        swarm_commits = [pick(0)] * 4 + [pick(1)] * 3 + [pick(5)] * 3 + [pick(14)] * 2 if num_worktrees == 12 else [c4[min(i * 4 // max(num_worktrees, 1), 3)] for i in range(num_worktrees)]

        if "t4" in stages or "t5" in stages:
            swarm_commits = validate_swarm_commits(src_dir, commits_desc, swarm_commits, ctx, extra_args, res)
        git_checkout_clean(src_dir, head_commit)
        try:
            res["project_rustc_version"] = subprocess.check_output(["rustc", "-vV"], cwd=src_dir, env=ctx["envs"]["cargo_bare"], text=True).strip().splitlines()[0]
        except Exception:
            res["project_rustc_version"] = "unknown"

        unique_commits = list(dict.fromkeys([head_commit] + replay_commits + [branch_hop_commit] + swarm_commits))
        commit_locks = prepare_commit_locks_and_fetch(src_dir, proj_work, ctx, head_commit, unique_commits, res)
        res["commits_selected"] = {
            "head": head_commit, "branch_hop": branch_hop_commit, "replay_count": len(replay_commits), "swarm_distinct": len(set(swarm_commits)),
            "replay_commits": [c[:12] for c in replay_commits], "swarm_commits": [c[:12] for c in swarm_commits],
        }

        if not run_stages_t0_t1_t2_t3(
            res, stages, src_dir, proj_work, proj_cache, ctx, mitts_bin, sccache_bin, extra_args,
            repeats, max_disk_mb, max_disk_gb, max_load, head_commit, branch_hop_commit, replay_commits, commit_locks,
            prewarm_commits=unique_commits, online_fetch=online_fetch,
        ):
            return res

        # ---------------- T4: L5a Check & L5b Clippy 12-WT Swarm + CoW Reflink Ablation ----------------
        if "t4" in stages:
            git_checkout_clean(src_dir, head_commit, commit_locks)
            chmod_rmtree(src_dir / "target")
            purge_mitts_cas(ctx["mitts_cas"], keep_sources=True)
            r_warm_cow = run_timed(tool_cmd("mitts_cas", "test_no_run", ctx, extra_args), cwd=src_dir, env=ctx["envs"]["mitts_cas"])
            if r_warm_cow["exit_code"] != 0:
                if cargo_also_fails(res, "test_no_run", src_dir, ctx, extra_args, "T4", r_warm_cow):
                    return res
                raise RuntimeError(f"T4 CoW ablation warmup failed: {r_warm_cow['stderr_tail']}")

            env_cow = {k: v for k, v in ctx["envs"]["mitts_cas"].items() if k != "MITTS_NO_REFLINK"}
            env_cow["MITTS_TIMINGS"] = "1"
            # The copy arm must copy bytes: without MITTS_STORE_HARDLINKS=0 the default `auto`
            # policy answers a disabled reflink with a read-only store hardlink instead.
            env_copy = {**ctx["envs"]["mitts_cas"], "MITTS_NO_REFLINK": "1", "MITTS_STORE_HARDLINKS": "0", "MITTS_TIMINGS": "1"}
            cow_runs, copy_runs = [], []
            cow_cmd = tool_cmd("mitts_cas", "test_no_run", ctx, extra_args)
            for it_idx in range(repeats):
                pairs = [(cow_runs, env_cow), (copy_runs, env_copy)] if it_idx % 2 == 0 else [(copy_runs, env_copy), (cow_runs, env_cow)]
                for bucket, env_m in pairs:
                    chmod_rmtree(src_dir / "target")
                    sync_and_wait_dirty(max_dirty_mb=1024.0, timeout_s=60.0)
                    _, b_before, rd_before = get_device_io_bytes([src_dir, proj_cache])
                    df_b = get_df_used_mb([proj_work, proj_cache])
                    r_ab = run_timed(cow_cmd, cwd=src_dir, env=env_m)
                    sync_and_wait_dirty(max_dirty_mb=1024.0, timeout_s=60.0)
                    _, b_after, rd_after = get_device_io_bytes([src_dir, proj_cache])
                    df_a = get_df_used_mb([proj_work, proj_cache])
                    r_ab["device_mb_written"] = round((b_after - b_before) / (1024.0 * 1024.0), 1) if (b_before is not None and b_after is not None) else None
                    r_ab["device_mb_read"] = round((rd_after - rd_before) / (1024.0 * 1024.0), 1) if (rd_before is not None and rd_after is not None) else None
                    r_ab["df_used_delta_mb"] = round(df_a - df_b, 1) if (df_b is not None and df_a is not None) else None
                    r_ab["io"] = parse_mitts_io(r_ab["output"])
                    bucket.append(r_ab)

            wt_root = proj_work / "t4_wts"
            snap_cas, snap_scc, snap_sh = proj_work / "t4_snap_cas", proj_work / "t4_snap_scc", proj_work / "t4_snap_sh"
            l5_res: dict = {"check": {}, "clippy": {}}
            for mode in ("check", "clippy"):
                try:
                    run_warmups_and_snapshot(mode, src_dir, wt_root, swarm_commits, commit_locks, ctx, extra_args, snap_cas, snap_scc, snap_sh)
                except RuntimeError as exc:
                    if str(exc).startswith("Mitts warmup"):
                        if mode == "clippy":
                            # A deny-by-default clippy lint at HEAD (blake3) fails under Cargo too:
                            # only the clippy swarm is lost, the check swarm and T5 still measure.
                            r_c = cargo_run_if_fails(mode, wt_root / "wt-01", ctx, extra_args)
                            if r_c is not None:
                                l5_res["clippy"] = {
                                    "skipped_upstream": True, "reason": "cargo clippy fails on the project's own code at HEAD",
                                    "cargo_exit_code": r_c["exit_code"], "cargo_stderr_tail": r_c["stderr_tail"][-3000:], "mitts_stderr_tail": str(exc)[-3000:],
                                }
                                res["status_flags"].append("L5B_SKIPPED_UPSTREAM")
                                print("  [t4/clippy] cargo clippy fails on the same tree: clippy swarm skipped as broken upstream, check swarm kept", flush=True)
                                continue
                        elif cargo_also_fails(res, mode, wt_root / "wt-01", ctx, extra_args, "T4", {"exit_code": None, "stderr_tail": str(exc)}):
                            return res
                    raise
                mode_series: dict = {t: [] for t in TOOLS}
                wt12_outputs: dict = {}
                wt12_root, anomaly_meta = src_dir, None

                for run_idx in range(1, repeats + 1):
                    shift = (run_idx - 1) % len(TOOLS)
                    for arm in TOOLS[shift:] + TOOLS[:shift]:
                        stop_sccache(sccache_bin, ctx["sccache_dir"])
                        load_before, _, waited_s, busy_before = wait_for_quiet_rig(timeout_s=300.0, max_load=max_load)
                        wts = prepare_swarm_worktrees(src_dir, wt_root, swarm_commits, commit_locks=commit_locks)
                        anomaly_meta = inject_anomaly(wts[-1][1], mode=mode, env=ctx["envs"]["cargo_bare"])
                        if arm == "mitts_cas":
                            restore_dir(snap_cas, ctx["mitts_cas"], keep_sources=True)
                        elif arm == "cargo_sccache":
                            restore_dir(snap_scc, ctx["sccache_dir"])
                            start_sccache_zero_stats(sccache_bin, ctx["envs"]["cargo_sccache"])
                        elif arm == "cargo_shared":
                            restore_dir(snap_sh, ctx["shared_target"])

                        m_sw = measure_swarm_arm(arm, mode, wts, wt_root, proj_work, proj_cache, ctx, extra_args, max_load)
                        runs, wall_m, dev_mb, df_delta, d_mb, scc_st, load_after, p_info = (m_sw[k] for k in SWARM_KEYS)
                        if d_mb > max_disk_mb:
                            res["status"] = f"SKIPPED_DISK_LIMIT_{max_disk_gb}GB"
                            res["disk_mb_observed"] = d_mb
                            return res

                        if run_idx == 1:
                            wt12_outputs[arm] = runs[-1]["output"]
                            if arm == "mitts_cas":
                                chmod_rmtree(wts[-1][1] / "target")
                                wt12_outputs["mitts_cached_replay"] = run_timed(tool_cmd(arm, mode, ctx, extra_args), wts[-1][1], ctx["envs"][arm])["output"]
                                wt12_root = wts[-1][1]

                        wt12_tail = "\n".join(runs[-1]["output"].strip().splitlines()[-30:]) if runs and runs[-1]["output"] else ""
                        chmod_rmtree(wt_root)
                        failures_m = sum(1 for r in runs if r["exit_code"] != 0)
                        print(f"  [t4/{mode}] run {run_idx}/{repeats} {arm}: swarm {wall_m} s, failures={failures_m}, waited {waited_s} s (cpu busy {busy_before}, load {load_before})", flush=True)
                        mode_series[arm].append({
                            "iteration": run_idx, "total_wall_s": wall_m, "failures": failures_m,
                            "wt12_exit_code": runs[-1]["exit_code"] if runs else None, "wt12_stderr_tail": wt12_tail,
                            "disk_mb": d_mb, "df_used_delta_mb": df_delta, "device_mb_written": dev_mb, "device_mb_read": m_sw["device_mb_read"],
                            "sccache_stats": scc_st, "resources": m_sw["resources"],
                            "loadavg_before": load_before, "loadavg_after": load_after, "cpu_busy_before": busy_before,
                            "preflight_waited_s": waited_s, "pressure": p_info,
                        })

                for arm, runs in mode_series.items():
                    l5_res[mode][arm] = {
                        **_swarm_summary(runs, "total_wall_s", "failures"), "failures": sum(r["failures"] for r in runs),
                        "iterations": runs,
                    }
                l5_res[mode]["diagnostics_parity"] = verify_diagnostics_parity(
                    wt12_outputs.get("cargo_bare", ""), wt12_outputs.get("mitts_cas", ""), wt12_outputs.get("mitts_cached_replay", ""), wt12_root, anomaly_info=anomaly_meta
                )
            # The T4 snapshots are not needed any more; T5 takes its own, and both sets
            # on disk at once pushed large projects over the disk limit.
            for snap in (snap_cas, snap_scc, snap_sh):
                chmod_rmtree(snap)

            def cow_samples(runs: list[dict]) -> list[dict]:
                return [{
                    "wall_s": r["wall_s"], "exit_code": r["exit_code"], **timed_resources(r), "device_mb_written": r.get("device_mb_written"),
                    "device_mb_read": r.get("device_mb_read"), "df_used_delta_mb": r.get("df_used_delta_mb"), "io": r.get("io", {}),
                } for r in runs]

            cow_rd = [r["device_mb_read"] for r in cow_runs if r.get("device_mb_read") is not None]
            copy_rd = [r["device_mb_read"] for r in copy_runs if r.get("device_mb_read") is not None]
            cow_io = parse_mitts_io(cow_runs[0]["output"]) if cow_runs else {}
            copy_io = parse_mitts_io(copy_runs[0]["output"]) if copy_runs else {}
            ref_eff = cow_io.get("io_stats", {}).get("bytes_cloned", 0) > 0
            cow_dev = [r["device_mb_written"] for r in cow_runs if r.get("device_mb_written") is not None]
            copy_dev = [r["device_mb_written"] for r in copy_runs if r.get("device_mb_written") is not None]
            cow_df = [r["df_used_delta_mb"] for r in cow_runs if r.get("df_used_delta_mb") is not None]
            copy_df = [r["df_used_delta_mb"] for r in copy_runs if r.get("df_used_delta_mb") is not None]
            t4_inv = evaluate_t4_invariants(l5_res)
            if not t4_inv["all_t4_sound_and_parity_ok"]:
                res["status_flags"].append("INVALID_L5")
                if res["status"] == "OK":
                    res["status"] = "INVALID_L5"
            res["stages"]["t4_check_clippy_and_cow"] = {
                "reflink_effective": ref_eff, "ablation_applicable": ref_eff,
                "cow_reflink_stats_s": compute_stats([r["wall_s"] for r in cow_runs if r["exit_code"] == 0]), "cow_io": cow_io,
                "cow_median_device_mb_written": compute_stats(cow_dev)["median"] if cow_dev else None,
                "cow_median_df_delta_mb": compute_stats(cow_df)["median"] if cow_df else None,
                "copy_fallback_stats_s": compute_stats([r["wall_s"] for r in copy_runs if r["exit_code"] == 0]), "copy_io": copy_io,
                "copy_median_device_mb_written": compute_stats(copy_dev)["median"] if copy_dev else None,
                "copy_median_df_delta_mb": compute_stats(copy_df)["median"] if copy_df else None,
                "cow_median_device_mb_read": compute_stats(cow_rd)["median"] if cow_rd else None,
                "copy_median_device_mb_read": compute_stats(copy_rd)["median"] if copy_rd else None,
                "cow_resources": resource_stats(cow_runs), "copy_resources": resource_stats(copy_runs),
                "cow_runs": cow_samples(cow_runs), "copy_runs": cow_samples(copy_runs),
                "invariants": t4_inv, "l5a_check_swarm": l5_res["check"], "l5b_clippy_swarm": l5_res["clippy"],
            }

        # ---------------- T5: 12-Worktree 4-Commit Test Build Swarm (Rotated Arms, Soundness & I/O) ----------------
        if "t5" in stages:
            wt_root = proj_work / "t5_wts"
            snap_cas, snap_scc, snap_sh = proj_work / "snap_cas", proj_work / "snap_scc", proj_work / "snap_sh"
            try:
                fast_lock_noop = run_warmups_and_snapshot("test_no_run", src_dir, wt_root, swarm_commits, commit_locks, ctx, extra_args, snap_cas, snap_scc, snap_sh)
            except RuntimeError as exc:
                if str(exc).startswith("Mitts warmup") and cargo_also_fails(
                    res, "test_no_run", wt_root / "wt-01", ctx, extra_args, "T5", {"exit_code": None, "stderr_tail": str(exc)}
                ):
                    return res
                raise
            unharnessed_cnt = select_workspace_targets(src_dir, env=ctx["envs"]["cargo_bare"])["unharnessed_test_targets"]

            arm_series: dict = {t: [] for t in TOOLS}
            for run_idx in range(1, repeats + 1):
                shift = (run_idx - 1) % len(TOOLS)
                for arm in TOOLS[shift:] + TOOLS[:shift]:
                    stop_sccache(sccache_bin, ctx["sccache_dir"])
                    load_before, _, waited_s, busy_before = wait_for_quiet_rig(timeout_s=300.0, max_load=max_load)
                    wts = prepare_swarm_worktrees(src_dir, wt_root, swarm_commits, commit_locks=commit_locks)
                    injected_rel = inject_test_defect(wts[-1][1], env=ctx["envs"]["cargo_bare"])
                    if arm == "mitts_cas":
                        restore_dir(snap_cas, ctx["mitts_cas"], keep_sources=True)
                    elif arm == "cargo_sccache":
                        restore_dir(snap_scc, ctx["sccache_dir"])
                        start_sccache_zero_stats(sccache_bin, ctx["envs"]["cargo_sccache"])
                    elif arm == "cargo_shared":
                        restore_dir(snap_sh, ctx["shared_target"])

                    m_sw = measure_swarm_arm(arm, "test_no_run", wts, wt_root, proj_work, proj_cache, ctx, extra_args, max_load)
                    wt_runs, wall_sw, dev_mb, df_delta, d_mb, scc_st, load_after, p_info = (m_sw[k] for k in SWARM_KEYS)
                    smoke_cmd = tool_cmd(arm, "test_smoke", ctx, extra_args)
                    s_wt01 = run_timed(smoke_cmd, cwd=wts[0][1], env=ctx["envs"][arm])
                    s_wt12 = run_timed(smoke_cmd, cwd=wts[-1][1], env=ctx["envs"][arm])
                    p_wt01, p_wt12 = parse_test_results(s_wt01["output"]), parse_test_results(s_wt12["output"])
                    wt_bins = [parse_build_output(r["output"], arm)["test_binaries_count"] for r in wt_runs]
                    chmod_rmtree(wt_root)

                    if d_mb > max_disk_mb:
                        res["status"] = f"SKIPPED_DISK_LIMIT_{max_disk_gb}GB"
                        res["disk_mb_observed"] = d_mb
                        return res

                    expected_min_exec = max(1, wt_bins[0] - unharnessed_cnt) if wt_bins[0] > 0 else 1
                    failed_tails = {f"wt-{idx:02d}": r["stderr_tail"] for (idx, _, _), r in zip(wts, wt_runs) if r["exit_code"] != 0}
                    build_failures = sum(1 for r in wt_runs if r["exit_code"] != 0)
                    print(f"  [t5] run {run_idx}/{repeats} {arm}: swarm {wall_sw} s, failures={build_failures}, binaries={wt_bins[0]}, waited {waited_s} s (cpu busy {busy_before}, load {load_before})", flush=True)
                    def smoke_record(r_sm: dict, p_sm: dict) -> dict:
                        return {
                            "wall_s": r_sm["wall_s"], "exit_code": r_sm["exit_code"], **timed_resources(r_sm),
                            "binaries_executed": p_sm["binaries_executed"], "passed": p_sm["passed"], "failed": p_sm["failed"],
                            "ignored": p_sm["ignored"], "failed_tests": p_sm["failed_tests"][:20],
                        }

                    arm_series[arm].append({
                        "iteration": run_idx, "total_wall_s": wall_sw, "wt01_wall_s": wt_runs[0]["wall_s"],
                        "build_failures": build_failures, "wt_binary_counts": wt_bins,
                        "failed_wt_stderr_tail": failed_tails,
                        "injected_defect_file": injected_rel, "disk_mb": d_mb, "df_used_delta_mb": df_delta,
                        "device_mb_written": dev_mb, "device_mb_read": m_sw["device_mb_read"], "sccache_stats": scc_st,
                        "resources": m_sw["resources"], "pressure": p_info,
                        "loadavg_before": load_before, "loadavg_after": load_after, "cpu_busy_before": busy_before,
                        "preflight_waited_s": waited_s,
                        "smoke_wt01": smoke_record(s_wt01, p_wt01), "smoke_wt12": smoke_record(s_wt12, p_wt12),
                        "smoke_wt01_clean_ok": s_wt01["exit_code"] == 0 and p_wt01["failed"] == 0 and p_wt01["binaries_executed"] >= expected_min_exec,
                        "smoke_wt12_defect_caught": s_wt12["exit_code"] != 0 and p_wt12["defect_reported"],
                    })

            for snap in (snap_cas, snap_scc, snap_sh):
                chmod_rmtree(snap)
            t5_summary = {}
            for arm, runs in arm_series.items():
                t5_summary[arm] = {**_swarm_summary(runs, "total_wall_s", "build_failures"), "iterations": runs}
            lib_trees, test_trees = set(), set()
            for c in set(swarm_commits):
                r_tr = subprocess.run(["git", "ls-tree", "-r", c], cwd=src_dir, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
                if r_tr.returncode == 0:
                    rs_lines = [ln for ln in r_tr.stdout.splitlines() if ln.endswith(".rs") and not ln.split("\t")[-1].startswith(("benches/", "examples/"))]
                    test_trees.add("\n".join(rs_lines))
                    lib_trees.add("\n".join(ln for ln in rs_lines if not ln.split("\t")[-1].startswith("tests/")))
            inv = evaluate_t5_invariants(arm_series, fast_lock_noop, unsound_testable=len(test_trees) > 1, swarm_lib_sources_distinct=len(lib_trees) > 1)
            if not inv["all_t5_sound_and_parity_ok"]:
                res["status_flags"].append("INVALID_T5")
            if not inv["sound_arms_total_wall_variance_le_15_pct"]:
                res["status_flags"].append("NOISY")
            for p_flag in ("INVALID_L5", "INVALID_T5", "NOISY"):
                if p_flag in res["status_flags"] and res["status"] in ("OK", "NOISY"):
                    res["status"] = p_flag
                    break
            m_med, b_med = t5_summary["mitts_cas"]["wall_stats_s"]["median"], t5_summary["cargo_bare"]["wall_stats_s"]["median"]
            s_med, sh_med = t5_summary["cargo_sccache"]["wall_stats_s"]["median"], t5_summary["cargo_shared"]["wall_stats_s"]["median"]
            res["stages"]["t5_worktree_swarm"] = {
                "worktrees": len(swarm_commits), "commits_distinct": len(set(swarm_commits)), "repeats": repeats,
                "swarm_commits": [c[:12] for c in swarm_commits], "unharnessed_test_targets": unharnessed_cnt,
                "arms": t5_summary, "invariants": inv,
                "speedup_vs_cargo_bare": round(b_med / m_med, 2) if (m_med and b_med) else None,
                "speedup_vs_cargo_sccache": round(s_med / m_med, 2) if (m_med and s_med) else None,
                "speedup_vs_cargo_shared": round(sh_med / m_med, 2) if (m_med and sh_med) else None,
                "cargo_shared_caveat": "unsound shared CARGO_TARGET_DIR across concurrent worktrees",
            }

    except Exception as exc:
        res["status"] = "ERROR"
        res["error"] = str(exc)
        res["traceback"] = traceback.format_exc()
    finally:
        stop_sccache(sccache_bin, ctx["sccache_dir"])
        gate_events = drain_gate_events()
        if gate_events:
            res["gate_events"] = gate_events
            if any(e["kind"] == "timeout" for e in gate_events):
                res["status_flags"].append("GATE_TIMEOUT")
                if res["status"] == "OK":
                    res["status"] = "NOISY"
        res["total_elapsed_s"] = round(time.perf_counter() - t_start, 2)
        chmod_rmtree(proj_work)
        chmod_rmtree(proj_cache)

    return res


def main() -> None:
    here = Path(__file__).resolve().parent
    ap = argparse.ArgumentParser(description="Unified Mitts T0->T5 Benchmark Runner (Linux XFS/ext4 & macOS APFS)")
    ap.add_argument("--repo", help="Single project name (from corpus_100.json), git URL, or local repository path")
    ap.add_argument("--projects-dir", help="Directory containing pre-cloned project folders (e.g. projects_100)")
    ap.add_argument("--corpus", default=str(here / "corpus_100.json"), help="JSON manifest of the 100 projects (default: scripts/corpus_100.json)")
    ap.add_argument("--work-dir", required=True, help="Directory for clones and git worktrees on target filesystem")
    ap.add_argument("--cache-dir", help="Directory for shared caches (mitts_cas, sccache, cargo_shared_target, cargo_home) on target filesystem (defaults to <work-dir>/caches)")
    ap.add_argument("--mitts-bin", required=True, help="Path to mitts release binary")
    ap.add_argument("--sccache-bin", default=shutil.which("sccache") or str(Path.home() / ".cargo/bin/sccache"), help="Path to sccache binary")
    ap.add_argument("--output", required=True, help="Output JSON file path (checkpointed after every project)")
    ap.add_argument("--commits", type=int, default=10, help="Number of consecutive first-parent commits for S3 Git History Replay (default: 10)")
    ap.add_argument("--worktrees", type=int, default=12, help="Number of concurrent git worktrees across 4 commits (default: 12)")
    ap.add_argument("--repeats", type=int, default=5, help="Iterations per series (default: 5)")
    ap.add_argument("--max-disk-gb", type=float, default=100.0, help="Skip/abort project if disk exceeds N GB (default: 100.0)")
    ap.add_argument("--max-busy", type=float, default=0.25, help="Maximum fraction of CPU busy (1 s sample) before starting a benchmark arm; a measured steady background above it raises the threshold to baseline + 0.10 (default: 0.25)")
    ap.add_argument("--baseline-samples", type=int, default=5, help="1 s CPU samples taken before the run to measure the rig's steady background load; 0 disables (default: 5)")
    ap.add_argument("--max-load", type=float, default=1.0, help="1m loadavg recorded with every run; gates only when the CPU sample is unavailable (default: 1.0)")
    ap.add_argument("--background-note", default="", help="Documented reason if --max-busy or --max-load is raised above the default")
    ap.add_argument("--stages", default="t0,t1,t2,t3,t4,t5", help="Comma-separated stages to execute (default: t0,t1,t2,t3,t4,t5)")
    ap.add_argument("--exclude", default="nushell", help="Comma-separated project names to exclude (default: nushell)")
    ap.add_argument("--allow-binary-mismatch", action="store_true", help="Allow resuming an output file created with different binary/scripts/parameters")
    ap.add_argument("--retry-noisy", action="store_true", help="On resume, measure NOISY projects again as well (by default only ERROR projects are redone)")
    ap.add_argument("--redo", default="", help="On resume, measure these comma-separated projects again whatever their status (an environmental failure such as a full disk)")
    ap.add_argument("--online-fetch", action="store_true", help="T1: also time one cold download per tool over the live network (empty registry index/cache and empty Mitts store)")
    args = ap.parse_args()

    work_dir = Path(args.work_dir).resolve()
    cache_dir = Path(args.cache_dir).resolve() if args.cache_dir else (work_dir / "caches")
    work_dir.mkdir(parents=True, exist_ok=True)
    cache_dir.mkdir(parents=True, exist_ok=True)
    mitts_bin, sccache_bin, out_path = Path(args.mitts_bin).resolve(), Path(args.sccache_bin).resolve(), Path(args.output).resolve()
    out_path.parent.mkdir(parents=True, exist_ok=True)
    stages = {s.strip().lower() for s in args.stages.split(",") if s.strip()}
    excluded = {x.strip() for x in args.exclude.split(",") if x.strip()}
    script_list = [here / f for f in ("benchmark_project.py", "benchmark_lib.py", "benchmark_stages.py", "benchmark_common.py", "benchmark_pressure.py", "benchmark_worktree.py", "benchmark_diagnostics.py", "t5_parsers.py", "corpus_100.json")]

    baseline_busy = measure_cpu_baseline(args.baseline_samples) if args.baseline_samples > 0 else None
    set_quiet_policy(args.max_busy, baseline_busy)
    print(f"[preflight] cpu busy baseline={baseline_busy} ({args.baseline_samples} x 1 s samples); quiet threshold={effective_max_busy()}", flush=True)
    cur_prov = collect_provenance(mitts_bin, sccache_bin, work_dir, cache_dir, script_list, args.max_load, args.background_note, max_busy=args.max_busy, baseline_busy=baseline_busy, effective_busy=effective_max_busy())
    report: dict = {
        "schema_version": "mitts_unified_t0_t5_v3",
        "provenance": cur_prov,
        "parameters": {"commits": args.commits, "worktrees": args.worktrees, "repeats": args.repeats, "max_disk_gb": args.max_disk_gb, "max_load": args.max_load, "max_busy": args.max_busy, "stages": sorted(stages), "excluded": sorted(excluded), "online_fetch": args.online_fetch},
        "projects": {},
    }
    if out_path.exists():
        existing = json.loads(out_path.read_text(encoding="utf-8"))
        old_prov = existing.get("provenance", {})
        # A measured project (OK, NOISY, INVALID_*) and a policy skip are reused on resume;
        # an ERROR project (and NOISY ones with --retry-noisy) is measured again from scratch.
        forced = {n.strip() for n in args.redo.split(",") if n.strip()}

        def redo(name: str, status: str | None) -> bool:
            if name in forced or status == "ERROR" or status is None:
                return True
            if args.retry_noisy and status == "NOISY":
                return True
            return not str(status).startswith(RESUME_KEEP_PREFIXES)

        stale = [n for n, p in existing.get("projects", {}).items() if redo(n, p.get("status"))]
        for name in stale:
            print(f"[resume] {name}: previous run ended {existing['projects'][name].get('status')}, starting over", flush=True)
            existing["projects"].pop(name)
        if existing.get("projects") and not args.allow_binary_mismatch:
            if old_prov.get("binary_sha256") and old_prov.get("binary_sha256") != cur_prov.get("binary_sha256"):
                sys.exit(f"Refusing to resume {out_path}: binary_sha256 mismatch")
            if old_prov.get("scripts_provenance") and old_prov.get("scripts_provenance") != cur_prov.get("scripts_provenance"):
                sys.exit(f"Refusing to resume {out_path}: scripts_provenance mismatch (pass --allow-binary-mismatch to override)")
            if existing.get("parameters") and existing.get("parameters") != report["parameters"]:
                sys.exit(f"Refusing to resume {out_path}: parameters mismatch (pass --allow-binary-mismatch to override)")
        report["projects"] = existing.get("projects", {})

    corpus_map = {p["name"]: p["url"] for p in json.loads(Path(args.corpus).read_text(encoding="utf-8")).get("projects", [])} if Path(args.corpus).is_file() else {}
    pdir = Path(args.projects_dir).resolve() if args.projects_dir else None
    if args.repo:
        pname = args.repo if args.repo in corpus_map else _project_name_from_spec(args.repo)
        spec = str(pdir / pname) if (pdir and (pdir / pname / "Cargo.toml").exists() and (pdir / pname / ".git").exists()) else corpus_map.get(args.repo, args.repo)
        targets = [(pname, spec)]
    elif pdir and pdir.is_dir():
        targets = [(p.name, str(p) if (p / ".git").exists() else corpus_map.get(p.name, str(p))) for p in sorted(pdir.iterdir()) if p.is_dir() and (p / "Cargo.toml").exists()]
    elif corpus_map:
        targets = [(k, v) for k, v in sorted(corpus_map.items())]
    else:
        sys.exit("Specify --repo, --projects-dir, or --corpus")

    for idx, (pname, spec) in enumerate(targets, 1):
        if pname in excluded:
            report["projects"][pname] = {"project": pname, "status": "EXCLUDED_POLICY"}
            out_path.write_text(json.dumps(report, indent=2), encoding="utf-8")
            continue
        if pname in report["projects"]:
            print(f"[{idx}/{len(targets)}] {pname}: already {report['projects'][pname].get('status')}, skipping", flush=True)
            continue

        print(f"[{idx}/{len(targets)}] Benchmarking {pname} ({spec})...", flush=True)
        r = benchmark_single_project(spec, work_dir, cache_dir, mitts_bin, sccache_bin, args.max_disk_gb, args.commits, args.worktrees, args.repeats, stages, args.max_load, proj_name=pname, online_fetch=args.online_fetch)
        report["projects"][pname] = r
        out_path.write_text(json.dumps(report, indent=2), encoding="utf-8")
        t5 = r.get("stages", {}).get("t5_worktree_swarm", {})
        print(f"  -> {pname}: status={r['status']} | T5 speedup vs bare={t5.get('speedup_vs_cargo_bare')}x vs sccache={t5.get('speedup_vs_cargo_sccache')}x ({r['total_elapsed_s']}s)", flush=True)


if __name__ == "__main__":
    main()
