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

from benchmark_common import wait_for_quiet_rig
from benchmark_lib import (
    TOOLS, chmod_rmtree, collect_provenance, copy_repo_tree, ensure_commit_history,
    evaluate_t4_invariants, evaluate_t5_invariants, git_checkout_clean, prepare_commit_locks_and_fetch,
    prepare_swarm_worktrees, purge_mitts_cas, restore_dir, run_timed, setup_environments,
    start_sccache_zero_stats, stop_sccache, tool_cmd, verify_diagnostics_parity,
)
from benchmark_pressure import get_device_bytes_written, get_df_used_mb
from benchmark_stages import measure_swarm_arm, run_stages_t0_t1_t2_t3, run_warmups_and_snapshot
from benchmark_worktree import inject_anomaly
from t5_parsers import compute_stats, inject_test_defect, parse_build_output, parse_mitts_io, parse_test_results, select_workspace_targets


def _project_name_from_spec(repo_spec: str) -> str:
    base = os.path.basename(repo_spec.rstrip("/"))
    return base[:-4] if base.endswith(".git") else base


def benchmark_single_project(
    repo_spec: str, work_dir: Path, cache_dir: Path, mitts_bin: Path, sccache_bin: Path,
    max_disk_gb: float, num_commits: int, num_worktrees: int, repeats: int,
    stages: set[str], max_load: float, proj_name: str | None = None,
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
    extra_args = ["--exclude", "tests-build"] if proj_name == "tokio" else []

    res: dict = {
        "project": proj_name,
        "status": "OK",
        "same_device": os.stat(proj_work).st_dev == os.stat(proj_cache).st_dev,
        "stages": {},
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
        head_commit = commits_desc[0]
        replay_commits = list(reversed(commits_desc[: min(num_commits, len(commits_desc))]))
        branch_hop_commit = commits_desc[min(14, len(commits_desc) - 1)]
        pick = lambda idx: commits_desc[idx] if len(commits_desc) > idx else commits_desc[-1]
        if num_worktrees == 12:
            swarm_commits = [pick(0)] * 4 + [pick(1)] * 3 + [pick(5)] * 3 + [pick(14)] * 2
        else:
            c4 = [pick(0), pick(1), pick(5), pick(14)]
            swarm_commits = [c4[min(i * 4 // max(num_worktrees, 1), 3)] for i in range(num_worktrees)]

        git_checkout_clean(src_dir, head_commit)
        try:
            res["project_rustc_version"] = subprocess.check_output(["rustc", "-vV"], cwd=src_dir, env=ctx["envs"]["cargo_bare"], text=True).strip().splitlines()[0]
        except Exception:
            res["project_rustc_version"] = "unknown"

        unique_commits = list(dict.fromkeys([head_commit] + replay_commits + [branch_hop_commit] + swarm_commits))
        commit_locks = prepare_commit_locks_and_fetch(src_dir, proj_work, ctx, head_commit, unique_commits, res)
        res["commits_selected"] = {"head": head_commit, "branch_hop": branch_hop_commit, "replay_count": len(replay_commits), "swarm_distinct": len(set(swarm_commits))}

        if not run_stages_t0_t1_t2_t3(
            res, stages, src_dir, proj_work, proj_cache, ctx, mitts_bin, sccache_bin, extra_args,
            repeats, max_disk_mb, max_disk_gb, max_load, head_commit, branch_hop_commit, replay_commits, commit_locks,
        ):
            return res

        # ---------------- T4: L5a Check & L5b Clippy 12-WT Swarm + CoW Reflink Ablation ----------------
        if "t4" in stages:
            git_checkout_clean(src_dir, head_commit, commit_locks)
            chmod_rmtree(src_dir / "target")
            purge_mitts_cas(ctx["mitts_cas"], keep_sources=True)
            r_warm_cow = run_timed(tool_cmd("mitts_cas", "test_no_run", ctx, extra_args), cwd=src_dir, env=ctx["envs"]["mitts_cas"])
            if r_warm_cow["exit_code"] != 0:
                raise RuntimeError(f"T4 CoW ablation warmup failed: {r_warm_cow['stderr_tail']}")

            env_cow = {k: v for k, v in ctx["envs"]["mitts_cas"].items() if k != "MITTS_NO_REFLINK"}
            env_cow["MITTS_TIMINGS"] = "1"
            env_copy = {**ctx["envs"]["mitts_cas"], "MITTS_NO_REFLINK": "1", "MITTS_TIMINGS": "1"}
            cow_runs, copy_runs = [], []
            cow_cmd = tool_cmd("mitts_cas", "test_no_run", ctx, extra_args)
            for it_idx in range(repeats):
                pairs = [(cow_runs, env_cow), (copy_runs, env_copy)] if it_idx % 2 == 0 else [(copy_runs, env_copy), (cow_runs, env_cow)]
                for bucket, env_m in pairs:
                    chmod_rmtree(src_dir / "target")
                    _, b_before = get_device_bytes_written([src_dir, proj_cache])
                    df_b = get_df_used_mb([proj_work, proj_cache])
                    r_ab = run_timed(cow_cmd, cwd=src_dir, env=env_m)
                    _, b_after = get_device_bytes_written([src_dir, proj_cache])
                    df_a = get_df_used_mb([proj_work, proj_cache])
                    r_ab["device_mb_written"] = round((b_after - b_before) / (1024.0 * 1024.0), 1) if (b_before is not None and b_after is not None) else None
                    r_ab["df_used_delta_mb"] = round(df_a - df_b, 1) if (df_b is not None and df_a is not None) else None
                    bucket.append(r_ab)

            wt_root = proj_work / "t4_wts"
            snap_cas, snap_scc, snap_sh = proj_work / "t4_snap_cas", proj_work / "t4_snap_scc", proj_work / "t4_snap_sh"
            l5_res: dict = {"check": {}, "clippy": {}}
            for mode in ("check", "clippy"):
                run_warmups_and_snapshot(mode, src_dir, wt_root, swarm_commits, commit_locks, ctx, extra_args, snap_cas, snap_scc, snap_sh)
                mode_series: dict = {t: [] for t in TOOLS}
                wt12_outputs: dict = {}
                wt12_root, anomaly_meta = src_dir, None

                for run_idx in range(1, repeats + 1):
                    shift = (run_idx - 1) % len(TOOLS)
                    for arm in TOOLS[shift:] + TOOLS[:shift]:
                        stop_sccache(sccache_bin, ctx["sccache_dir"])
                        load_before, _, waited_s = wait_for_quiet_rig(timeout_s=300.0, max_load=max_load)
                        wts = prepare_swarm_worktrees(src_dir, wt_root, swarm_commits, commit_locks=commit_locks)
                        anomaly_meta = inject_anomaly(wts[-1][1], mode=mode, env=ctx["envs"]["cargo_bare"])
                        if arm == "mitts_cas":
                            restore_dir(snap_cas, ctx["mitts_cas"], keep_sources=True)
                        elif arm == "cargo_sccache":
                            restore_dir(snap_scc, ctx["sccache_dir"])
                            start_sccache_zero_stats(sccache_bin, ctx["envs"]["cargo_sccache"])
                        elif arm == "cargo_shared":
                            restore_dir(snap_sh, ctx["shared_target"])

                        runs, wall_m, dev_mb, df_delta, d_mb, scc_st, load_after, p_info = measure_swarm_arm(
                            arm, mode, wts, wt_root, proj_work, proj_cache, ctx, extra_args, max_load
                        )
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
                        mode_series[arm].append({
                            "iteration": run_idx, "total_wall_s": wall_m, "failures": sum(1 for r in runs if r["exit_code"] != 0),
                            "wt12_exit_code": runs[-1]["exit_code"] if runs else None, "wt12_stderr_tail": wt12_tail,
                            "disk_mb": d_mb, "df_used_delta_mb": df_delta, "device_mb_written": dev_mb, "sccache_stats": scc_st,
                            "loadavg_before": load_before, "loadavg_after": load_after, "preflight_waited_s": waited_s, "pressure": p_info,
                        })

                for arm, runs in mode_series.items():
                    ok_w = [r["total_wall_s"] for r in runs if r["failures"] == 0]
                    dev_vals = [r["device_mb_written"] for r in runs if r["device_mb_written"] is not None]
                    l5_res[mode][arm] = {
                        "wall_stats_s": compute_stats(ok_w), "failures": sum(r["failures"] for r in runs),
                        "max_disk_mb": max((r["disk_mb"] for r in runs), default=0.0),
                        "median_device_mb_written": compute_stats(dev_vals)["median"] if dev_vals else None,
                        "iterations": runs,
                    }
                l5_res[mode]["diagnostics_parity"] = verify_diagnostics_parity(
                    wt12_outputs.get("cargo_bare", ""), wt12_outputs.get("mitts_cas", ""), wt12_outputs.get("mitts_cached_replay", ""), wt12_root, anomaly_info=anomaly_meta
                )

            cow_io = parse_mitts_io(cow_runs[0]["output"]) if cow_runs else {}
            copy_io = parse_mitts_io(copy_runs[0]["output"]) if copy_runs else {}
            ref_eff = cow_io.get("io_stats", {}).get("bytes_cloned", 0) > 0
            cow_dev = [r["device_mb_written"] for r in cow_runs if r.get("device_mb_written") is not None]
            copy_dev = [r["device_mb_written"] for r in copy_runs if r.get("device_mb_written") is not None]
            cow_df = [r["df_used_delta_mb"] for r in cow_runs if r.get("df_used_delta_mb") is not None]
            copy_df = [r["df_used_delta_mb"] for r in copy_runs if r.get("df_used_delta_mb") is not None]
            t4_inv = evaluate_t4_invariants(l5_res)
            if not t4_inv["all_t4_sound_and_parity_ok"] and res["status"] == "OK":
                res["status"] = "INVALID_L5"
            res["stages"]["t4_check_clippy_and_cow"] = {
                "reflink_effective": ref_eff, "ablation_applicable": ref_eff,
                "cow_reflink_stats_s": compute_stats([r["wall_s"] for r in cow_runs if r["exit_code"] == 0]),
                "cow_io": cow_io,
                "cow_median_device_mb_written": compute_stats(cow_dev)["median"] if cow_dev else None,
                "cow_median_df_delta_mb": compute_stats(cow_df)["median"] if cow_df else None,
                "copy_fallback_stats_s": compute_stats([r["wall_s"] for r in copy_runs if r["exit_code"] == 0]),
                "copy_io": copy_io,
                "copy_median_device_mb_written": compute_stats(copy_dev)["median"] if copy_dev else None,
                "copy_median_df_delta_mb": compute_stats(copy_df)["median"] if copy_df else None,
                "invariants": t4_inv, "l5a_check_swarm": l5_res["check"], "l5b_clippy_swarm": l5_res["clippy"],
            }

        # ---------------- T5: 12-Worktree 4-Commit Test Build Swarm (Rotated Arms, Soundness & I/O) ----------------
        if "t5" in stages:
            wt_root = proj_work / "t5_wts"
            snap_cas, snap_scc, snap_sh = proj_work / "snap_cas", proj_work / "snap_scc", proj_work / "snap_sh"
            fast_lock_noop = run_warmups_and_snapshot("test_no_run", src_dir, wt_root, swarm_commits, commit_locks, ctx, extra_args, snap_cas, snap_scc, snap_sh)
            unharnessed_cnt = select_workspace_targets(src_dir, env=ctx["envs"]["cargo_bare"])["unharnessed_test_targets"]

            arm_series: dict = {t: [] for t in TOOLS}
            for run_idx in range(1, repeats + 1):
                shift = (run_idx - 1) % len(TOOLS)
                for arm in TOOLS[shift:] + TOOLS[:shift]:
                    stop_sccache(sccache_bin, ctx["sccache_dir"])
                    load_before, _, waited_s = wait_for_quiet_rig(timeout_s=300.0, max_load=max_load)
                    wts = prepare_swarm_worktrees(src_dir, wt_root, swarm_commits, commit_locks=commit_locks)
                    injected_rel = inject_test_defect(wts[-1][1], env=ctx["envs"]["cargo_bare"])
                    if arm == "mitts_cas":
                        restore_dir(snap_cas, ctx["mitts_cas"], keep_sources=True)
                    elif arm == "cargo_sccache":
                        restore_dir(snap_scc, ctx["sccache_dir"])
                        start_sccache_zero_stats(sccache_bin, ctx["envs"]["cargo_sccache"])
                    elif arm == "cargo_shared":
                        restore_dir(snap_sh, ctx["shared_target"])

                    wt_runs, wall_sw, dev_mb, df_delta, d_mb, scc_st, load_after, p_info = measure_swarm_arm(
                        arm, "test_no_run", wts, wt_root, proj_work, proj_cache, ctx, extra_args, max_load
                    )
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
                    arm_series[arm].append({
                        "iteration": run_idx, "total_wall_s": wall_sw, "wt01_wall_s": wt_runs[0]["wall_s"],
                        "build_failures": sum(1 for r in wt_runs if r["exit_code"] != 0), "wt_binary_counts": wt_bins,
                        "failed_wt_stderr_tail": failed_tails,
                        "injected_defect_file": injected_rel, "disk_mb": d_mb, "df_used_delta_mb": df_delta,
                        "device_mb_written": dev_mb, "sccache_stats": scc_st, "pressure": p_info,
                        "loadavg_before": load_before, "loadavg_after": load_after, "preflight_waited_s": waited_s,
                        "smoke_wt01_clean_ok": s_wt01["exit_code"] == 0 and p_wt01["failed"] == 0 and p_wt01["binaries_executed"] >= expected_min_exec,
                        "smoke_wt12_defect_caught": s_wt12["exit_code"] != 0 and p_wt12["defect_reported"],
                    })

            t5_summary = {}
            for arm, runs in arm_series.items():
                ok_w = [r["total_wall_s"] for r in runs if r["build_failures"] == 0]
                dev_vals = [r["device_mb_written"] for r in runs if r["device_mb_written"] is not None]
                t5_summary[arm] = {
                    "wall_stats_s": compute_stats(ok_w), "max_disk_mb": max((r["disk_mb"] for r in runs), default=0.0),
                    "median_device_mb_written": compute_stats(dev_vals)["median"] if dev_vals else None, "iterations": runs,
                }
            lib_trees, test_trees = set(), set()
            for c in set(swarm_commits):
                r_tr = subprocess.run(["git", "ls-tree", "-r", c], cwd=src_dir, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
                if r_tr.returncode == 0:
                    rs_lines = [ln for ln in r_tr.stdout.splitlines() if ln.endswith(".rs") and not ln.split("\t")[-1].startswith(("benches/", "examples/"))]
                    test_trees.add("\n".join(rs_lines))
                    lib_trees.add("\n".join(ln for ln in rs_lines if not ln.split("\t")[-1].startswith("tests/")))
            inv = evaluate_t5_invariants(arm_series, fast_lock_noop, unsound_testable=len(test_trees) > 1, swarm_lib_sources_distinct=len(lib_trees) > 1)
            if not inv["all_t5_sound_and_parity_ok"] and res["status"] in ("OK", "NOISY"):
                res["status"] = "INVALID_T5"
            elif not inv["sound_arms_total_wall_variance_le_15_pct"] and res["status"] == "OK":
                res["status"] = "NOISY"
            m_med, b_med = t5_summary["mitts_cas"]["wall_stats_s"]["median"], t5_summary["cargo_bare"]["wall_stats_s"]["median"]
            s_med, sh_med = t5_summary["cargo_sccache"]["wall_stats_s"]["median"], t5_summary["cargo_shared"]["wall_stats_s"]["median"]
            res["stages"]["t5_worktree_swarm"] = {
                "worktrees": len(swarm_commits), "commits_distinct": len(set(swarm_commits)), "repeats": repeats,
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
    ap.add_argument("--max-load", type=float, default=1.0, help="Maximum 1m loadavg before starting a benchmark arm (default: 1.0)")
    ap.add_argument("--background-note", default="", help="Documented reason if --max-load > 1.0 is used")
    ap.add_argument("--stages", default="t0,t1,t2,t3,t4,t5", help="Comma-separated stages to execute (default: t0,t1,t2,t3,t4,t5)")
    ap.add_argument("--exclude", default="nushell", help="Comma-separated project names to exclude (default: nushell)")
    ap.add_argument("--allow-binary-mismatch", action="store_true", help="Allow resuming an output file created with different binary/scripts/parameters")
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

    cur_prov = collect_provenance(mitts_bin, sccache_bin, work_dir, cache_dir, script_list, args.max_load, args.background_note)
    report: dict = {
        "schema_version": "mitts_unified_t0_t5_v3",
        "provenance": cur_prov,
        "parameters": {"commits": args.commits, "worktrees": args.worktrees, "repeats": args.repeats, "max_disk_gb": args.max_disk_gb, "max_load": args.max_load, "stages": sorted(stages), "excluded": sorted(excluded)},
        "projects": {},
    }
    if out_path.exists():
        existing = json.loads(out_path.read_text(encoding="utf-8"))
        old_prov = existing.get("provenance", {})
        if not args.allow_binary_mismatch:
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
        if pname in report["projects"] and report["projects"][pname].get("status") == "OK":
            print(f"[{idx}/{len(targets)}] {pname}: already completed, skipping", flush=True)
            continue

        print(f"[{idx}/{len(targets)}] Benchmarking {pname} ({spec})...", flush=True)
        r = benchmark_single_project(spec, work_dir, cache_dir, mitts_bin, sccache_bin, args.max_disk_gb, args.commits, args.worktrees, args.repeats, stages, args.max_load, proj_name=pname)
        report["projects"][pname] = r
        out_path.write_text(json.dumps(report, indent=2), encoding="utf-8")
        t5 = r.get("stages", {}).get("t5_worktree_swarm", {})
        print(f"  -> {pname}: status={r['status']} | T5 speedup vs bare={t5.get('speedup_vs_cargo_bare')}x vs sccache={t5.get('speedup_vs_cargo_sccache')}x ({r['total_elapsed_s']}s)", flush=True)


if __name__ == "__main__":
    main()
